# Platform Worklist

This is the only active platform worklist. Design notes, evidence ledgers,
runbooks, and operator notes are inputs, not parallel trackers. Historical
implementation diaries remain in docs/worklist-archive/ and are not executable
tasks.

## Current Snapshot - 2026-08-19 fully automated production 13.0.0 execution plus feature completion

- **9 active epics:** 8 `Remaining`, 0 `Blocked`, 1 `Awaiting testing`, 0 `Needs clarification`.
  Operator 2026-08-29: close Remaining source epics one at a time, features
  and services first; do not add extra farm gates. `WL-FUNC-023` archived
  2026-08-30. `WL-FUNC-024` through `WL-FUNC-032` archived 2026-08-29. Operator 2026-08-27 moved all live-seat, release-wait, and
  operator-testing leftovers to `WL-TEST-003` (`Awaiting testing` until a testing
  Beta is released).
  Operator 2026-08-28 skipped Construct Health Fix: do not fan or wait on a
  DRM Fix click; that leftover executes on `WL-TEST-003` only after the
  Test Release. Dest-cut `bc14a22d7` is not that Beta. REL dest-operator
  admission stays on Remaining REL epics. Do not invent a mesh-id or bearer.
  Do not flip `production_admitted`. Unpublished `13.0.0-35` / lighthouse
  `13.0.0-11` (`bc14a22d7`) remains installed on the dest-cut set.
- **Latest stable integration:** 43 exact hostile gates passed across four farm hosts: `evidence/WORKLIST-2026-08-11-stable-exact-wave-r473.md`.
- **Execution order:** lifecycle source is archived (`WL-FUNC-023`);
  record one clean pushed release-candidate revision and epoch
  under `WL-REL-001` S1; materialize and admit the already-selected production
  inputs under `WL-REL-006` against that exact candidate; reconfirm that the
  candidate did not move and promote the same revision to the final source
  freeze; cut and sign the six roles under `WL-REL-002`/`WL-REL-003`; stage
  and release a testing Beta; then execute live-seat and operator testing
  under `WL-TEST-003`. Farm fixture gates stay on `WL-TEST-002`. Complete the
  final signed evidence envelope under `WL-REL-004`; then publish and read
  back under `WL-REL-005`. If source changes
  after input generation begins, invalidate the source-bound receipts and
  repeat input   admission; never solve the dependency by weakening source
  binding.
- **Feature-completion lane (2026-08-19):** `WL-FUNC-024` through `WL-FUNC-032`
  close the remaining gap between implementation-complete and fit for purpose —
  the Communications parity-ledger rulings that never landed, the Calls media
  plane, and the operator-flagged legacy mesh-PBX retirement. They are
  implementation-only, disjoint from the release chain, and pre-freeze source
  work executable in parallel by disjoint workers. Live-seat and operator
  testing for those epics is `WL-TEST-003` after a testing Beta.
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
  artifact/package proof, and farm fixture gates are owned by `WL-TEST-002`.
  Installed-seat, provider, live, and operator-testing leftovers are owned by
  `WL-TEST-003` and execute only after a testing Beta is released. Product
  epics must not duplicate those rollout tasks; they retain implementation
  gaps and cite `WL-TEST-003` when live acceptance is a dependency.
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

Close every remaining epic one at a time, starting with real features and
services. Reuse a fresh HEAD farm result; do not grind extra crates or
`cargo test --workspace` as filler. When an operator choice is required,
apply documented best practice and continue. Live-seat leftovers stay on
`WL-TEST-003` until a testing Beta exists. After source-complete feature
epics are archived (`WL-FUNC-023` on 2026-08-30), resume the release
chain from the same exact clean revision.

## Service Release Queue

1. Unified ONBOARD & OFFBOARDING lifecycle (archived 2026-08-30).
2. Create and admit real production release inputs.
3. Re-freeze the feature-complete `13.0.0` source on the protected default branch.
4. Build and self-sign all six canonical roles.
5. Stage the exact unpublished candidate on the six-node production topology.
6. Release a testing Beta.
7. Execute live-seat and operator testing under `WL-TEST-003`.
8. Assemble and sign the final provenance/evidence bundle, then publish
   `magic-mesh-v13.0.0`, verify readback, and complete the staged fleet handoff.

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
stories is a process failure, not a resource shortage. After every
commit/push, start `automation/reconciler/tick-fill.sh` (or
`systemctl start --no-block mcnf-farm-reconcile.service`);
`mcnf-farm-reconcile.path` starts the same oneshot on HEAD change. Do not
wait for the 15-min timer. Do not hand-fan a cargo command the reconciler
already owns. When cargo is fresh at the current clean HEAD, the next act
is `automation/drain/leftover-units.sh runnable` (source leftovers only
until a testing Beta exists). Live-seat, release-wait, operator-testing,
and Construct Health Fix leftovers live on `WL-TEST-003` and are not
runnable while it is Awaiting testing. Do not stall the drain on a DRM Fix click.
`@leftover:{dest-operator}` / `keep` / `release-wait` on Remaining epics do
not fill slots; they do not authorize invented dests.

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

- Status: Remaining
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
- Current state: `WL-FUNC-023` archived 2026-08-30. S1 contracts and S4
  Android deferral projection landed. S3 / REL-006 dest-operator leftovers
  stay parked. Unpublished `13.0.0-35` is dest-cut, not a testing Beta.
  Do not invent dests or flip `production_admitted`. Evidence:
  `WL-REL-007-2026-08-30-android-deferred-r1.md`. Exact acceptance is
  Dell, Seat 15, Surface, and three lighthouses.
- Remaining work:
  1. S1 Establish SOL Luna execution ownership and release ordering.
     Complete: `automation/promotion/release-intent.py` validates
     `ReleaseIntentV1`/`ReleaseStateV1`; drafts stay unadmitted;
     Cuttlefish, invented dests, and `production_admitted` flips refuse.
     Journal receipts remain `release-stage-journal.sh`. Evidence:
     `WL-REL-007-2026-08-30-release-intent-r1.md`. Signed admission on
     protected `master` stays dest-operator leftover.
  2. S2 Complete the unified lifecycle under WL-FUNC-023.
     Complete: archived 2026-08-30. Official `cargo test -p mackesd` passed
     5187/0/1 at `519c415bc`. Evidence:
     `WL-FUNC-023-2026-08-30-source-close-r1.md`. Live leftovers remain
     `WL-TEST-003` after a testing Beta.
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
     Complete: preflight/inventory already refuse Android inputs. Shared
     lifecycle view now projects `android: Deferred` and does not treat
     android/cuttlefish warnings as a readiness gate. Evidence:
     `WL-REL-007-2026-08-30-android-deferred-r1.md`. Do not launch or
     provision Android for `13.0.0`.
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
       by `WL-TEST-003` after a testing Beta.
     - Validation: tested bytes match the candidate; DRM, audio, HID, sensor,
       camera, power, Cast/DLNA, provider, and guest evidence is captured by
       objective fixtures; every failure recovers by corrected-forward action
       or re-enrollment, never rollback or manual assertion.
     - Done when: `WL-TEST-003` live S1-S7 pass after a testing Beta, or
       reopen one exact owning implementation blocker with no invented success.
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
     - Action: complete `WL-TEST-003` live S8 after the testing Beta; map every obligation to evidence or a
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
- Dependencies: `WL-FUNC-023` archived; parked WL-REL-006 leftovers; then the
  remaining release chain in S1-S8 order. A failed gate reopens its owning
  story rather than requesting interactive resolution.
- Acceptance criteria: one clean source produces exactly six signed roles;
  real governed inputs pass preflight; production topology passes; signed evidence and public
  readback agree; no fixture, rollback, filler build, or fabricated proof
  satisfies a gate.
- Verification method: worklist lint, focused hostile tests, meaningful gates
  across all permanent farm hosts, exact three-seat and
  three-lighthouse acceptance, signed evidence verification, clean-room public
  readback, and repository query. @farm:{cargo metadata --format-version 1}
  @leftover:{dest-operator}
- Origin or merged source IDs: SOL Luna AI completion plan and Android deferral
  direction (2026-08-17).

### WL-REL-001 - Freeze the newest feature-complete release source

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: production `13.0.0` is newer than the latest published tag, and loose historical artifacts do not define one admissible release source.
- Required outcome: freeze one clean, pushed, feature-complete `13.0.0` commit
  on the protected default branch and bind every release input, version surface,
  note, and tag plan to it.
- Current state: dest-cut SHA is 7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac
  epoch 1787450205 (maps-verifier lock refresh after 2872293b1 `--locked`
  refuse). Unpublished signed dest is bound; this is not the final freeze.
  Superseded 1dfe6906 must not receive new inputs. S2 13.0.0 metadata remains
  `WL-REL-001-2026-08-16-version-metadata-farm-r1.md` plus the named
  2026-08-23 matrix (`WL-REL-001-2026-08-23-version-matrix-r1.md`;
  `check-release-version-surfaces.sh` PASS). Brand epoch `13` maps to
  Construct (`WL-REL-001-2026-08-23-construct-epoch-13-r1.md`; farm
  `mde-theme` `22/22`). Live FUNC-023 enroll leftover is `WL-TEST-003`
  after a testing Beta. Final S1/S4 freeze still needs REL-006 admission
  and dest-cut SHA reconfirmation. Do not declare the final freeze until
  that dest-operator leftover closes.
- Remaining work:
  1. S1 Select the immutable source. Candidate recorded. Live FUNC-023 enroll
     leftover is WL-TEST-003 after a testing Beta: 2872293b1 / 1787447942 is
     the input-generation candidate (`WL-REL-001-2026-08-22-input-candidate-r1.md`).
     Operator 2026-08-22 marked PR #71 Ready and authorized this record. Final
     freeze still waits on REL-006 admission and reconfirmation, not live-seat
     leftover. Superseded 1dfe6906 must not receive new inputs.
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
     candidate 2872293b1 / 1787447942. Superseded 1dfe6906 must not receive
     new release inputs. Maps
     approval/source, App VM image/catalog receipt,
     bootc receipt is not admitted for the frozen revision. Android/Cuttlefish
     is deferred and must not appear in the `13.0.0` preflight object. Maps provider/live proof
     is explicitly deferred to WL-TEST-003 after a testing Beta; that deferral does not create a
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
- Dependencies: `WL-FUNC-023` is archived; WL-REL-006 must complete against
  the S1 candidate before final S1/S4
  freeze disposition. This is a two-phase reconfirmation, not a circular
  requirement for two source revisions. Signed release intent authorizes
  self-signing; exact installed/live proof remains deferred to WL-TEST-003.
- Acceptance criteria: one clean pushed revision is frozen; all version surfaces and inputs bind to it; stale artifacts cannot enter later stages.
- Verification method: local read-only Git/version checks, focused farm metadata/package checks, preflight admission, and evidence review.
  @farm:{cargo metadata --format-version 1}
  @leftover:{dest-operator}
- Origin or merged source IDs: release recovery of archived WL-BUILD-001, WL-BUILD-003, and WL-CRIT-006 responsibilities.

### WL-REL-006 - Create governed open-source release inputs

- Status: Remaining
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
  Catalog choice is dest-backed Flathub LibreOffice (office guest), not a
  parked ref. Evidence:
  `WL-REL-006-2026-08-23-flathub-catalog-chosen-r1.md`. Leftover is Maps
  `production_admitted`, RPM signer after freeze, S7 Maps/RPM `REPLACE_*`,
  Surface `bootc_base` null. Do not flip `production_admitted`.
- Remaining work:
  1. S1 Establish the open-source input policy.
     - Inputs: candidate source receipt, Fedora target, architecture, applicable
       licenses, and the existing receipt/verifier contracts.
     - Action: six-role redacted inventory is recorded; do not reopen source
       selection. Browser VM Containerfile pin now matches the admitted index
       `3a5e74e6…`.        Leftover is Maps `production_admitted`, RPM signer after freeze,
       S7 Maps/RPM `REPLACE_*`, Surface `bootc_base`, and live-seat dest
       (WL-TEST-003 after a testing Beta). App catalog choice is Flathub LibreOffice dest.
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
       (WL-TEST-003 after a testing Beta). Clipped PBF is not
       MBTiles admission. Fixture PNG raster is not production
       admission. Preserve PBF, boundary, clip,
       renderer/style/font identities, ODbL attribution, aggregate quota,
       and deterministic transport. Never fetch public OSM tiles; defer
       installed runtime proof to WL-TEST-003.
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
       `3a5e74e6…` (`WL-REL-006-2026-08-22-app-vm-base-pin-r1.md`).
       Catalog choice is dest-backed Flathub LibreOffice. Leftover is S7
       Maps/RPM `REPLACE_*`; do not claim the release-input gate closed.
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
- Dependencies: WL-FUNC-023 source/cargo before freeze; WL-REL-001 S1 candidate
  identity and S2 version matrix; operator-approved curated Flatpak refs; the
  governed RPM signer secret after freeze; WL-TEST-003 for live-seat dest after
  a testing Beta.
  Do not invent catalog refs. Do not guess Surface `bootc_base` while blocked.
- Acceptance criteria: every mandatory first-release input is reproducible,
  licensed, immutable, current-revision-bound, and admitted by preflight; no
  fixture, unavailable input, or external handoff can satisfy the gate.
- Verification method: farm-only source/image/package gates, receipt inspectors,
  hostile substitution tests, license review, and canonical preflight.
  @farm:{cargo build --workspace}
  @leftover:{dest-operator}
- Origin or merged source IDs: WL-CRIT-006, WL-FUNC-017, WL-FUNC-018,
  WL-FUNC-020, and the deferred WL-TEST-003 provider-proof queue.

### WL-REL-002 - Cut the complete three-RPM unsigned handoff

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: the release needs same-revision Workstation, Server, and Lighthouse RPMs; the loose artifact store has no admissible complete set.
- Required outcome: build exactly three Fedora 44 RPM roles from the WL-REL-001 source and publish one immutable private production-candidate handoff.
- Current state: unpublished signed 13.0.0 dest is bound
  (`WL-REL-002-2026-08-22-unpublished-cut-sign-r1.md`). Native F44 builder
  `172.20.0.131` is up (toolchain-ready;
  `WL-REL-002-2026-08-22-f44-builder-recover-r1.md`). BigBoy F42 `.130` is
  halted for that RAM handoff. Official prepare still needs Maps/catalog
  `REPLACE_*`. Not freeze. Operator 2026-08-23 authorized Remaining; live
  FUNC-023 enroll leftover is WL-TEST-003 after a testing Beta. Official
  cut still needs REL-006 admission.
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
  @farm:{cargo build -p mackesd}
- Origin or merged source IDs: archived WL-BUILD-001 and first-release preparation from WL-CRIT-006.

### WL-REL-003 - Self-sign RPMs and produce all derivative release roles

- Status: Remaining
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
  @farm:{cargo test -p mackesd}
  @leftover:{dest-operator}
- Origin or merged source IDs: archived WL-BUILD-001, WL-BUILD-003, WL-FUNC-016, WL-FUNC-017, and WL-CRIT-006 release roles.

### WL-REL-004 - Assemble the signed six-role release evidence bundle

- Status: Remaining
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
- Dependencies: WL-REL-003. Live prepublication S1-S7 is WL-TEST-003 after a
  testing Beta; do not wait on live-seat leftover to assemble the bundle.
- Acceptance criteria: one signed immutable six-role evidence bundle passes all mandatory gates and rejects any artifact-set drift.
- Verification method: farm gates, collector and gate verifiers, SBOM/evidence checks, detached-signature verification, and publication preflight.
  @farm:{cargo test -p mde-bus}
- Origin or merged source IDs: archived WL-BUILD-003 and WL-CRIT-006 production-evidence responsibilities.

### WL-REL-005 - Publish and promote the newest complete release

- Status: Remaining
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
     - Action: update WL-TEST-003 with exact release inputs and select exactly Dell, Seat 15, and Surface as physical proof seats.
     - Deliverable: acceptance handoff naming immutable artifacts, seats, lighthouses, providers, and rollback-forbidden recovery plan.
     - Validation: all references resolve and every seat mutation requires the governed alert/wait sequence.
     - Done when: WL-TEST-003 live S8 can compare public bytes with the already
       qualified private candidate without guessing any identity.
- Scope: tag, GitHub release, asset readback, signed package metadata promotion, and acceptance handoff.
- Relevant files/components: Git remote/tag tooling, GitHub release workflow, verify-github-release-binding.sh,
  packaging/repo, dnf-channel helpers, release notes, and WL-TEST-003.
- Dependencies: WL-REL-004.
- Acceptance criteria: tag, release, assets, signatures, provenance, and package metadata agree exactly; no partial release is visible.
- Verification method: remote tag/release readback, clean-room asset verification, repository queries, and HOLD/partial promotion refusal.
  @farm:{cargo test -p mde-enroll}
- Origin or merged source IDs: archived WL-BUILD-001, WL-BUILD-003, and WL-CRIT-006 publication responsibilities.

### WL-TEST-002 - Install and prove the newest complete release

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: exact-release installation, providers, direct-DRM rendering, guest/device integrations, and corrected-forward recovery need live proof.
- Required outcome: qualify the exact unpublished production candidate on Dell,
  Seat 15, and Surface, prove the three-lighthouse topology, then verify the
  same bytes after WL-REL-005 publication. Eagle and T480 remain non-gating
  inspection/deployment-wave seats.
- Current state: dest-cut `bc14a22d7` workstation `13.0.0-35` on Dell,
  Seat 15, Surface; lighthouse `13.0.0-11` on LH1–LH3. Eagle/T480 remain
  non-gating prior dest-cut. Compute/observation run after identity
  `Wants=`. Collab receipt SHA dest-gated. Leftover is S6 + providers.
  Operator 2026-08-23 authorized Remaining; not six-role qualification.
  Sealed Vitelity/SIP still required for live S3. No feature waiver. Live
  S1-S8 leftover moved to WL-TEST-003 after a testing Beta. Evidence:
  `WL-FUNC-023-2026-08-25-destcut-bc14a22d7-r1.md`.
- Remaining work: farm fixture gates only. Live S1-S8 and operator testing
  execute on WL-TEST-003 after a testing Beta; do not fan live-seat here.
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
  @farm:{cargo test -p mde-shell-egui}
- Origin or merged source IDs: WL-TEST-001 proof boundary and deferred queues from archived UX, Music, Collaboration, guest, and recovery epics.

### WL-TEST-003 - Execute live-seat and operator testing after a testing Beta

- Status: Awaiting testing
- Priority: P1
- Complexity: Epic
- Problem: live-seat proofs, release-wait leftovers, and operator testing were
  attached to Remaining source and release epics, so the drain executed them
  before a testing Beta existed.
- Required outcome: after a testing Beta is released, execute every transferred
  live-seat, release-wait, and operator-testing leftover on Dell, Seat 15,
  Surface, and the three lighthouses. Do not invent dests. Do not flip
  `production_admitted`.
- Current state: Operator 2026-08-27 moved those leftovers here. Dest-cut
  `bc14a22d7` (`13.0.0-35` / LH `13.0.0-11`) is not a testing Beta. This epic
  stays Awaiting testing until a testing Beta is released. leftover-units
  ignores Awaiting testing and Blocked bodies, so drain must not fan
  live-seat now. Operator 2026-08-28
  skipped Construct Health Fix until that Test Release exists.
- Remaining work: do not execute until a testing Beta is released. Then:
  1. S1 Live lifecycle leftover from archived WL-FUNC-023 (Construct
     Health Fix click on the DRM seat, dest-gated arming/Browser VM/collab
     SHA, enroll/offboard proofs). Close evidence:
     `WL-FUNC-023-2026-08-30-source-close-r1.md`.
  2. S2 Exact installed qualification that was WL-TEST-002 S1-S8.
  3. S3 Operator live proofs that were WL-FUNC-024 through WL-FUNC-032
     (calls media, Files POSIX/prefs/bookmarks, Transfers, Fleet voice
     including Vitelity dest, SIP gateway, co-edit, hotkeys).
  4. S4 Release-wait leftovers that were on WL-REL-007 and WL-REL-001
     through WL-REL-005. After the testing Beta, reconcile those against
     live evidence here; do not re-attach leftover markers to Remaining
     source epics.
- Scope: live-seat, release-wait, and operator testing only. Source, cargo,
  and dest-operator admission for freeze/inputs stay on their owning epics.
- Relevant files/components: Dell, Seat 15, Surface, LH1–LH3; Construct;
  mackesd lifecycle; Communications/Files; release evidence helpers.
- Dependencies: a published testing Beta (not dest-cut-only unpublished
  candidate). Then the three-seat `13.0.0` qualification topology.
- Acceptance criteria: every transferred leftover has live evidence or a
  reopened owning implementation story; no invented dest; no
  `production_admitted` flip from this epic.
- Verification method: after Status becomes Remaining, farm fixtures then
  live three-seat checks. @farm:{cargo test -p mde-shell-egui}
  @leftover:{live-seat} @leftover:{release-wait} @leftover:{dest-operator}
- Origin or merged source IDs: operator 2026-08-27 leftover restructure;
  operator 2026-08-28 skipped Construct Health Fix until the Test Release;
  WL-TEST-002 live queue; WL-FUNC-023/024–032 live leftovers; REL
  release-wait leftovers.

## Feature Completion

These epics close the remaining gap between implementation-complete and fit
for purpose: the Communications parity-ledger rulings that never landed and
the Calls media plane. `WL-FUNC-023` archived 2026-08-30. Feature-completion
epics `WL-FUNC-024` through `WL-FUNC-032` archived 2026-08-29. They were
implementation-only and disjoint from the release chain; each rides the
story execution contract above.

