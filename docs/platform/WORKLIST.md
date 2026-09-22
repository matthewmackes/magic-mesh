# Platform Worklist

This is the only active platform worklist. Design notes, evidence ledgers,
runbooks, and operator notes are inputs, not parallel trackers. Historical
implementation diaries remain in docs/worklist-archive/ and are not executable
tasks.

## Current Snapshot - 2026-09-22 critical-path finish of production 13.0.0

- **9 active epics:** 3 `Remaining`, 5 `Blocked`, 1 `Awaiting testing`, 0 `Needs clarification`.
  Feature source is archived (`WL-FUNC-023` on 2026-08-30; `WL-FUNC-024` through
  `WL-FUNC-032` on 2026-08-29). This file is now the release-finish tracker, not a
  feature drain. Operator 2026-08-29 one-at-a-time source close still applies:
  execute the one unblocked source leftover; do not add extra farm gates.
  Operator 2026-08-31 authorized dest-cut freeze. Protected `master` is
  `42035dcbd` / `1788153988`. S7 preflight passed at that SHA. Do not invent a
  dest, mesh-id, or bearer. Do not flip `production_admitted`.
- **Latest stable integration:** 43 exact hostile gates passed across four farm hosts: `evidence/WORKLIST-2026-08-11-stable-exact-wave-r473.md`.
- **Critical path (execute this, then flip the next Blocked epic to Remaining):**
  1. `WL-REL-003` S4/S6 — build Browser VM and App VM derivatives once, then write
     the six-role plan input. This is the only Remaining source leftover.
  2. `WL-REL-004` — collect, gate, SBOM, and sign the six-role envelope.
  3. `WL-REL-005` — tag, publish, clean-room readback, then a testing Beta.
  4. `WL-TEST-003` — live-seat and operator testing after that Beta.
  5. `WL-REL-007` S8 — archive the plan after public readback agrees.
- **Parallel Remaining (does not unblock publication):** `WL-TEST-002` farm
  fixture gates only. Do not fan live-seat here.
- **Parked (do not fan to fill slots):**
  - `WL-REL-001` — `github-required` failed. Drain-branch patched rustls
    0.23.45, h2 0.4.16, cryptoki 0.12.1. Remaining deny leftovers are
    yanked `chacha20` 0.10.1 (now 0.10.2) and `quick-xml` 0.30.0 (patched
    off `zbus_xml`). `chacha20` 0.9.1 remains and is not yanked. Official
    RPM secret inspect PASS on `.130`
    (`WL-REL-001-2026-09-22-rpm-secret-inspect-r1.md`).
  - `WL-REL-006` — dest-operator leftovers closed; `production_admitted` is
    fail-closed; in-tree Surface stack stays blocked.
  - `WL-REL-002` — unsigned freeze handoff exists; native F44 `.131` is halted
    and must not RAM-steal `.130` while `WL-REL-003` S4 runs.
  - `WL-REL-004` / `WL-REL-005` — predecessor not green.
  - `WL-TEST-003` — Awaiting a testing Beta. Dest-cut `bc14a22d7` and freeze-SHA
    seat installs are not that Beta.
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
- **Android deferral lock (2026-08-17):** all Android and Cuttlefish capability is
  deferred beyond `13.0.0`. The production set is exactly six roles: Workstation
  RPM, Server RPM, Lighthouse RPM, Browser VM, App VM, and bootc image.
- **Shared release-proof ownership:** farm fixture gates stay on `WL-TEST-002`.
  Installed-seat, provider, live, and operator-testing leftovers stay on
  `WL-TEST-003` and execute only after a testing Beta. Product epics must not
  duplicate those rollout tasks.
- **Production qualification topology (release lock 2026-08-16):** deep
  acceptance for `13.0.0` is exactly Seat 15, Dell, and Surface. Eagle and T480
  are non-gating inspection/deployment-wave seats. Three lighthouses remain
  independently required. ARM64 remains outside the envelope.
- **Privacy-retention lock (release lock 2026-08-10):** system logs, Bus
  history, transfer ledgers, collaboration JSONL, application histories, and
  audit records have a fleet-wide maximum lifetime of six hours.
- **Farm lock:** heavy verification is farm-only; route the longest job to
  BigBoy at 172.20.0.130. Five hosts / ten heavy slots (`.50` cap 2, `.90` cap 2,
  `.130` cap 3, `.170` cap 2, `.196` cap 1). Never run filler tests. Official
  workspace grind is not a Remaining unit on this file.
- **Rollout lock:** prove each release activity on exactly Dell, Seat 15, and
  Surface plus the independently required lighthouses. Publish the red
  AI-GENERATED-ALERT and wait five seconds before each mutation. Recover by
  re-enrollment and corrected-forward deployment, never rollback.
- **Objective-qualification lock:** replace human listening and visual review
  with hash-bound machine observations. A missing fixture fails its owning gate.
- **Story format:** execute Remaining stories top-to-bottom. Do not start a
  Blocked story. If a dependency is absent, keep the epic Blocked with the exact
  owning story and machine-readable failed gate; do not invent evidence.

## Active Drain Goal

Finish production `13.0.0` by executing the one unblocked source leftover
(`WL-REL-003` S4/S6), keeping farm fixture gates green on `WL-TEST-002`, and
leaving parked lanes parked. Reuse a fresh HEAD farm result. Do not grind
`cargo test --workspace` or `cargo build --workspace`. Do not fan
dest-operator, live-seat, or release-wait leftovers. After `WL-REL-003` S6
passes, flip `WL-REL-004` to Remaining and continue the chain.

## Service Release Queue

1. Build Browser VM and App VM derivatives and the six-role plan (`WL-REL-003`).
2. Assemble and sign the six-role evidence bundle (`WL-REL-004`).
3. Publish `magic-mesh-v13.0.0`, read back, and cut a testing Beta (`WL-REL-005`).
4. Execute live-seat and operator testing (`WL-TEST-003`).
5. Archive the coordinator after public readback agrees (`WL-REL-007` S8).

Parked beside that queue, not in it: `WL-REL-001` (`github-required` failed),
`WL-REL-006` (fail-closed `production_admitted`), `WL-REL-002` (native F44
`.131` halted).

## Story execution contract

Every story below is a self-contained unit. The implementing agent must:
read the named inputs; change only the owned files; produce the named deliverable;
add the stated hostile or regression test; run the stated validation; record the
revision, command, result, and evidence path; and mark the story complete only
when the Done when condition is true. A passing compile without the named
behavioral evidence is not completion.

Every `Status: Remaining` epic below carries at least one
`@farm:{cargo …}` payload as part of `Verification method:`. That payload is
what `automation/lib/farm-jobs.sh active` emits. Multiple markers on the same
epic authorise disjoint parallel workers. Adding or retiring an epic changes
the queue; do not build a second queue (`AI_GOVERNANCE.md` §10.0.4).

`Status: Blocked` and `Status: Awaiting testing` epics are intentionally
absent from `farm-jobs.sh active` and `leftover-units.sh`. Do not flip them to
Remaining to occupy a free slot.

## Parallel drain execution contract

The canonical tick is `install-helpers/drain-coordinator.sh plan` or
`automation/drain/ship-coordinator.sh --once`. Both read
`install-helpers/farm-topology.sh` and `automation/lib/farm-jobs.sh active`.

While Remaining epics exist, keep `min(active_farm_jobs, free_slots)` slots
busy on those Remaining units only. After every commit/push, start
`automation/reconciler/tick-fill.sh`. Do not wait for the 15-min timer. Do not
hand-fan a cargo command the reconciler already owns. When cargo is fresh at
the current clean HEAD, the next act is
`automation/drain/leftover-units.sh runnable` — today that is
`@leftover:{source}` on `WL-REL-003`. Live-seat leftovers live on
`WL-TEST-003` and are not runnable. `@leftover:{dest-operator}` /
`keep` / `release-wait` do not fill slots and do not authorize invented dests.

Local heavy `cargo` remains blocked by
`install-helpers/install-drain-guardrails.sh` (exit 97). Do not bypass.

## Non-stall execution contract

- A blocked story parks only its dependent lane. `WL-REL-007` remains
  Remaining and names the next unblocked owning epic. The drain does not stop
  for status reporting.
- Preserve owned dirty work on the current branch; never discard it to obtain
  a clean receipt.
- Credential preflight loads named systemd/mde-seal credentials, tests them
  without disclosure, and retries at 30 seconds, 2 minutes, and 10 minutes. A
  still-failing credential parks its live/publication lane.
- Native Fedora 44 `.131` is provisioned only when it will not RAM-steal
  `.130` from `WL-REL-003` S4. Container-F44 output is compatibility evidence,
  not the production RPM leftover.

### WL-REL-007 - Execute the SOL Luna AI production 13.0.0 completion plan

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: the remaining release epics form one production chain, but agents
  were treating every later stage as Remaining and grinding filler cargo.
- Required outcome: one restart-safe coordinator drives the owning epics to
  produce, qualify, publish, read back, and archive production
  `magic-mesh-v13.0.0` from one exact clean protected-default-branch revision,
  with exactly six canonical roles and no fabricated evidence.
- Current state: `WL-FUNC-023` archived 2026-08-30. S1 and S4 landed. Operator
  2026-08-31 authorized dest-cut freeze. Protected `master` is `42035dcbd` /
  `1788153988`. S7 preflight passed at that SHA. Maps dest operator-admitted.
  Surface dest pin is private selected `3a5e74e6…`; in-tree stack stays
  blocked. `github-required` failed. Unadmitted `ReleaseIntentV1` draft
  exists privately for `42035dcbd`; admission stays dest-operator.
  Next owning work is `WL-REL-003` S4/S6: dest-cut RPMs admitted on `.130`;
  App VM image lacked `cpio`. Native-F44 local bytes do not match S3
  manifests. Evidence: `WL-REL-003-2026-09-22-s4-derivatives-42035dcbd-r2.md`.
- Remaining work:
  1. S1 Complete except dest-operator admission: contracts refuse invented
     dests; private unadmitted draft exists at
     `/root/mcnf-private/release-intent-42035dcbd.json`. Do not invent a
     signature. Evidence: `WL-REL-001-2026-09-22-signing-and-tls-r1.md`.
  2. S2 Complete: `WL-FUNC-023` archived 2026-08-30.
  3. S3 Parked on `WL-REL-006`: dest-operator leftovers closed without new dests;
     `production_admitted` stays fail-closed.
  4. S4 Complete: Android/Cuttlefish deferred across release contracts.
     Evidence: `WL-REL-007-2026-08-30-android-deferred-r1.md`.
  5. S5 Next act: execute `WL-REL-003` S4/S6 on freeze SHA `42035dcbd`. Then
     reconfirm `WL-REL-001` after `github-required` is green. If the
     freeze tree moves, invalidate receipts and return to `WL-REL-006`.
  6. S6 Parked on `WL-TEST-003` until a testing Beta exists.
  7. S7 Parked on `WL-REL-004` then `WL-REL-005`.
  8. S8 After public readback, archive every completed owning epic and this
     coordinator in a post-release documentation commit so the frozen revision
     does not move.
- Scope: coordination and dependency enforcement. Owning epics remain the sole
  owners of implementation and acceptance work.
- Relevant files/components: `docs/platform/WORKLIST.md`, release/farm helpers,
  OpenTofu farm declarations, release input producers, packaging, evidence
  collectors, and publication verifiers.
- Dependencies: next unblocked owner is `WL-REL-003`. Parked owners are
  `WL-REL-001`, `WL-REL-002`, `WL-REL-006`, `WL-REL-004`, `WL-REL-005`, and
  `WL-TEST-003`.
- Acceptance criteria: one clean source produces exactly six signed roles;
  real governed inputs pass preflight; production topology passes; signed
  evidence and public readback agree; no fixture or fabricated proof satisfies
  a gate.
- Verification method: worklist lint plus the owning epic's farm unit. Do not
  treat this coordinator metadata command as a workspace grind.
  @farm:{cargo metadata --format-version 1}
  @leftover:{source}
- Origin or merged source IDs: SOL Luna AI completion plan and Android deferral
  direction (2026-08-17); 2026-09-22 critical-path restructure.

### WL-REL-003 - Self-sign RPMs and produce all derivative release roles

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: a complete release requires three signed RPM roles and three verified
  image roles; Browser VM and App VM derivatives and the six-role plan are the
  remaining source gap on freeze SHA `42035dcbd`.
- Required outcome: self-sign the exact handoff RPMs without changing payload
  identity and produce Browser VM, App VM, and bootc roles.
- Current state: freeze-SHA S1–S3 signed RPMs and candidate manifests exist on
  BigBoy. S5 dest-cut bootc receipt inspect PASS. Browser VM base receipt
  recovered from Fedora registry at dest-cut digest `3a5e74e6…`
  (`WL-REL-003-2026-08-31-browser-base-receipt-42035dcbd-r1.md`); did not
  follow moved quay `:44`. App VM base inspect PASS. 2026-09-22 S4 dest-cut
  RPM admission on `.130` PASS (wrapper + workstation `9f78ec2b…` /
  lighthouse `c4057d9c…` match S3). Helper then REFUSED inside the App VM
  image: dest-cut Fedora base lacks `cpio` before `verify-rpm-supply.sh`.
  Evidence: `WL-REL-003-2026-09-22-s4-derivatives-42035dcbd-r2.md`. Do not
  start `.131`.
- Remaining work:
  1. S1 Complete: governed fingerprint `06B1C27EA0E08A225155EB3314018AA1497DDC7C`
     selected; keyring destroyed after sign.
  2. S2 Complete: three RPMs signed without payload drift. Evidence:
     `WL-REL-003-2026-08-31-prepare-rpms-42035dcbd-r1.md`.
  3. S3 Complete: RPM candidate manifests and App/Browser base receipts.
     Evidence: `WL-REL-003-2026-08-31-s3-candidates-42035dcbd-r1.md`.
  4. S4 Build Browser VM and App VM derivatives.
     - Inputs: signed Workstation/Lighthouse RPMs, candidate manifests, dest-cut
       base receipts at `3a5e74e6…`, and App catalog inputs.
     - Action: run `build-release-derivative-images.sh` exactly once with an
       absent private output path, then construct the release-output plan from
       those exact files; never collect an earlier pair and silently rebuild.
     - Deliverable: immutable Browser VM and App VM images, manifests, and
       frozen Browser profile.
     - Validation: image manifest verifiers, qcow2 checks, source revision
       checks, and hostile substitution fixture.
     - Done when: both derivatives verify and the helper publishes no partial
       output. Failed gate 2026-09-22 r2: dest-cut App VM base image has no
       `cpio`; Containerfile now installs it before RPM supply verify.
  5. S5 Complete: bootc dest-cut receipt inspect PASS. Evidence:
     `WL-REL-003-2026-08-31-s5-bootc-inspect-42035dcbd-r1.md`.
  6. S6 Create the exact six-role plan input.
     - Inputs: three signed RPMs/manifests, two derivative images/manifests, and
       bootc fields.
     - Action: write one private `mcnf-release-output-plan-input` JSON object
       containing exactly the six canonical roles.
     - Deliverable: immutable plan input and a redacted role inventory.
     - Validation: `produce-release-output-plan.py` accepts it; missing,
       duplicate, extra, relative, mutable, or cross-revision inputs refuse.
     - Done when: exactly six role records are accepted and no artifact path is
       ambiguous.
- Scope: self-signing, candidate manifests, derivative generation, and plan
  input; no final evidence signing, publication, or installation.
- Relevant files/components: install-helpers/sign-release.sh,
  install-helpers/build-release-derivative-images.sh,
  install-helpers/produce-release-output-plan.py, packaging/app-vm,
  packaging/browser-vm, and bootc receipt tools.
- Dependencies: freeze-SHA signed RPMs already exist. Do not reopen
  `WL-REL-002` native F44 while S4 needs `.130`.
- Acceptance criteria: three RPM signatures verify without payload drift; three
  image roles verify; exactly six roles bind to `42035dcbd`.
- Verification method: signing and role-specific verifiers, derivative hostile
  suite, plan producer, and independent hash/identity comparison.
  @farm:{cargo test -p mackesd}
  @leftover:{source}
- Origin or merged source IDs: archived WL-BUILD-001, WL-BUILD-003, WL-FUNC-016,
  WL-FUNC-017, and WL-CRIT-006 release roles.

### WL-TEST-002 - Install and prove the newest complete release

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: farm fixture gates for the freeze-SHA candidate must stay green
  without pulling live-seat leftovers back onto a Remaining source epic.
- Required outcome: keep first-release farm fixture gates honest at
  `42035dcbd`. Live qualification of Dell, Seat 15, Surface, and the three
  lighthouses is `WL-TEST-003` after a testing Beta.
- Current state: native F44 freeze SHA `42035dcbd` workstation `13.0.0-35`
  on Dell, Seat 15, Surface (replaced dest-cut `bc14a22d7` same NVR).
  Lighthouse `13.0.0-11` on LH1–LH3 is still the prior dest-cut, not this
  native cut. Eagle/T480 were not mutated. Operator 2026-08-23 authorized
  Remaining; not six-role qualification. Live S1-S8 leftover moved to
  `WL-TEST-003`. Evidence:
  `WL-TEST-002-2026-08-31-native-f44-seat-install-42035dcbd-r1.md`.
- Remaining work: farm fixture gates only. Do not fan live-seat, providers,
  DRM capture, or guest install here.
  1. Keep `cargo test -p mde-shell-egui` green at freeze SHA `42035dcbd`.
  2. Record fixture regressions against this epic; reopen a named
     implementation story if a fixture fails. Do not waive a missing fixture.
  3. After `WL-REL-003` S6 exists, bind fixture identity to the six-role
     candidate bytes; do not invent a second candidate.
- Scope: farm fixture gates and unpublished-candidate identity checks only.
- Relevant files/components: docs/platform/release-evidence, install-helpers
  release/live/recovery verifiers, packaging installed-identity tools, and
  archived epic dispositions.
- Dependencies: freeze SHA `42035dcbd`. Live S1-S8 is `WL-TEST-003`.
- Acceptance criteria: farm fixtures pass at the freeze SHA; live leftovers
  remain on `WL-TEST-003`; no fixture pass is treated as installed-seat proof.
- Verification method: focused farm fixture gates. Live three-seat checks stay
  on `WL-TEST-003`.
  @farm:{cargo test -p mde-shell-egui}
- Origin or merged source IDs: WL-TEST-001 proof boundary and deferred queues
  from archived UX, Music, Collaboration, guest, and recovery epics.

### WL-REL-002 - Cut the complete three-RPM unsigned handoff

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: the release needs same-revision Workstation, Server, and Lighthouse
  RPMs; the native F44 production leftover is not the already-cut container
  handoff.
- Required outcome: build exactly three Fedora 44 RPM roles from the
  `WL-REL-001` source and publish one immutable private production-candidate
  handoff.
- Current state: freeze-SHA unsigned handoff exists on BigBoy
  (`/home/mm/mcnf-unsigned-handoff-42035dcbd`, promotion forbidden;
  `WL-REL-002-2026-08-31-unsigned-handoff-42035dcbd-r1.md`). Official
  `run-first-full-release.sh prepare` PASS at dest-cut via container-F44 on
  `.130`. Later native F44 workstation+lighthouse RPMs were installed on
  seats (`WL-TEST-002-2026-08-31-native-f44-seat-install-42035dcbd-r1.md`);
  Server was not recut natively. Native F44 builder `172.20.0.131` is halted.
- Remaining work: do not execute while `WL-REL-003` S4 needs `.130`.
  Unblock when `.131` can run without RAM-stealing `.130`, then:
  1. S1 Reconfirm frozen source and admit native Fedora 44 on `.131`.
  2. S2–S3 Build Workstation, Lighthouse, and Server on native F44.
  3. S4 Seal `handoff.json` from those native bytes.
  Container-F44 output remains compatibility evidence, not this leftover.
- Scope: unsigned RPM construction and immutable handoff only.
- Relevant files/components: install-helpers/run-first-full-release.sh,
  install-helpers/xcp-build.sh, packaging/app-vm, packaging/server-rpm,
  packaging/browser-vm.
- Dependencies: blocked on native F44 `.131` capacity that does not halt
  `.130`. Failed gate: `172.20.0.131` halted after prior RAM handoff.
- Acceptance criteria: one immutable three-role native F44 handoff exists;
  every RPM is exact and same-source; partial or substituted sets refuse.
- Verification method: after Status becomes Remaining, BigBoy native F44 RPM
  lane, independent Server lane, and handoff hostile verification.
  Unblock payload: `@farm:{cargo build -p mackesd}`.
- Origin or merged source IDs: archived WL-BUILD-001 and first-release
  preparation from WL-CRIT-006.

### WL-REL-006 - Create governed open-source release inputs

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: `WL-REL-001` cannot close until current-revision receipts stay bound
  to `42035dcbd` and helpers still refuse a self-marked `production_admitted`.
- Required outcome: keep the already-selected Maps, App VM, bootc, and UX-014
  inputs bound to the freeze SHA. Fixtures cannot satisfy a production gate.
- Current state: Operator 2026-08-31 authorized dest-cut freeze. Frozen source
  `42035dcbd` is on `master`. Maps dest `6d01a543…` operator-admitted. Catalog
  is Flathub LibreOffice. RPM identity inspect matched `06B1C27EA0…` at
  dest-cut. Selected bootc dest `3a5e74e6…` is privately pinned for Surface;
  in-tree stack stays blocked. S7 preflight passed at dest-cut. Do not rebind
  dests off the freeze SHA. Evidence:
  `WL-REL-001-2026-08-31-source-freeze-r1.md`.
- Remaining work: do not fan dest-operator or invent dests. S1–S7 source
  producers already ran at `42035dcbd`. RPM public-key bind of
  `rpm-signing-identity-42035dcbd.json` to
  `packaging/repo/RPM-GPG-KEY-magic-mesh` PASS. Official secret inspect
  waits on XEN-BIGBOY. Parked leftovers:
  1. Helpers refuse to self-mark `production_admitted` (fail-closed).
  2. In-tree Surface `bootc_base` stays blocked (unsigned Surface RPMs).
  3. Live-seat dest is `WL-TEST-003` after a testing Beta.
  Unblock only if the freeze tree moves (invalidate and regenerate) or a named
  dest-operator input is authorized for an existing dest.
- Scope: open-source source selection, receipts, licenses, and preflight
  admission; no public release or live-seat testing.
- Relevant files/components: install-helpers/release-input-preflight.sh,
  packaging/app-vm, install-helpers/produce-bootc-digest-receipt.py, Maps
  catalog/verifier tools, and the Kiron asset verifier.
- Dependencies: blocked on fail-closed `production_admitted` and in-tree
  Surface stack. Failed gate: helpers refuse self-admission; Surface
  `surface-stack.f44.json` unsigned.
- Acceptance criteria: every mandatory first-release input stays reproducible,
  licensed, immutable, and freeze-SHA-bound; no fixture satisfies the gate.
- Verification method: after Status becomes Remaining, receipt inspectors,
  hostile substitution tests, and canonical preflight.
  Unblock payload: `@farm:{cargo metadata --format-version 1}`.
- Origin or merged source IDs: WL-CRIT-006, WL-FUNC-017, WL-FUNC-018,
  WL-FUNC-020, and the deferred WL-TEST-003 provider-proof queue.

### WL-REL-001 - Freeze the newest feature-complete release source

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: production `13.0.0` needs one admissible freeze, and
  `github-required` at that revision failed.
- Required outcome: freeze one clean, pushed, feature-complete `13.0.0` commit
  on the protected default branch and bind every release input, version
  surface, note, and tag plan to it.
- Current state: frozen source is `42035dcbd76b03b8323399892052b21a96e2e233`
  epoch `1788153988` on protected `master`. S7 preflight passed; S2 version
  matrix is complete; S4 notes drafted
  (`WL-REL-001-2026-08-31-s4-notes-draft-r1.md`). Drain-branch docs after
  dest-cut are not the freeze tree. Nightly `35685970709` failed these
  exact upstream jobs: farm-gate policy, rustfmt `found.rs`, fedora-native
  and coverage (`mde-collab-core` blob root-open), cargo-deny licenses plus
  advisories. Drain-branch now has rustls 0.23.45, h2 0.4.16, cryptoki
  0.12.1. Yanked `chacha20` 0.10.1 and `quick-xml` 0.30.0 dropped; `chacha20`
  0.9.1 remains and is not yanked. Official RPM secret inspect PASS.
  Evidence: `WL-REL-001-2026-09-22-signing-and-tls-r1.md`,
  `WL-REL-001-2026-09-22-rpm-secret-inspect-r1.md`.
- Remaining work: do not tag or publish. Do not generate new dests.
  1. S1 Candidate recorded as `42035dcbd` / `1788153988`. Final freeze
     disposition waits for `github-required` pass at that exact SHA.
  2. S2 Complete: shipped surfaces resolve to 13.0.0.
  3. S3 Preflight passed at dest-cut; parked on `WL-REL-006` fail-closed
     `production_admitted`, not on missing product choices.
  4. S4 Notes drafted; close only when notes, tag plan, source receipt, and
     input inventory agree on the same frozen revision after `github-required`.
- Scope: source identity, version authority, mandatory input admission, release
  notes, and tag planning only; no artifact build or publication.
- Relevant files/components: Cargo.toml, Cargo.lock, isolated Cargo workspaces,
  docs/RELEASE-VERSIONING.md, install-helpers/source-revision-receipt.sh,
  install-helpers/release-input-preflight.sh.
- Dependencies: blocked on required check `github-required` at `42035dcbd`.
  Drain-branch yanked leftovers closed; `lru` advisory via `ratatui` remains.
  Official RPM secret inspect PASS on `.130`.
  Do not treat a job with that name that is not required as release authority.
- Acceptance criteria: one clean pushed revision is frozen; all version
  surfaces and inputs bind to it; stale artifacts cannot enter later stages.
- Verification method: after Status becomes Remaining, local Git/version
  checks, preflight admission, and evidence review.
  Unblock payload: `@farm:{cargo metadata --format-version 1}`.
- Origin or merged source IDs: release recovery of archived WL-BUILD-001,
  WL-BUILD-003, and WL-CRIT-006 responsibilities.

### WL-REL-004 - Assemble the signed six-role release evidence bundle

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: publication is forbidden until all artifacts, manifests, gates,
  SBOM data, checksums, and provenance form one exact signed bundle.
- Required outcome: collect and verify all six roles, execute mandatory
  release gates, and sign one immutable publication envelope.
- Current state: historical seven-role plan and collector pass for private
  historical `afc24782` preview. That collection is promotion-forbidden and
  is not freeze SHA `42035dcbd`. Waiting on `WL-REL-003` S4/S6. Evidence:
  `docs/platform/evidence/WL-REL-003-WL-REL-004-preview-afc-r1.md`.
- Remaining work: do not start until `WL-REL-003` S6 accepts exactly six
  freeze-SHA roles. Then:
  1. S1 Resume and collect the six-role output into an absent private path.
  2. S2 Execute the canonical gate matrix for `42035dcbd`.
  3. S3 Generate aggregate six-role SBOM/license and evidence envelope.
  4. S4 Sign checksums and provenance with `sign-release.sh --evidence`.
  5. S5 Preflight remote publication without publishing.
- Scope: evidence collection, release gates, SBOM, provenance, signatures,
  and publication preflight; no public mutation or seat installation.
- Relevant files/components: install-helpers/run-first-full-release.sh,
  produce-release-output-plan.py, collect-release-outputs.py,
  release-gate-matrix.json, verify-release-gate-matrix.py, sign-release.sh.
- Dependencies: blocked on `WL-REL-003` S6. Failed gate: six-role plan input
  for `42035dcbd` does not exist. Live prepublication is `WL-TEST-003`.
- Acceptance criteria: one signed immutable six-role evidence bundle passes
  all mandatory gates and rejects any artifact-set drift.
- Verification method: after Status becomes Remaining, farm gates, collector
  and gate verifiers, SBOM/evidence checks, and publication preflight.
  Unblock payload: `@farm:{cargo test -p mde-bus}`.
- Origin or merged source IDs: archived WL-BUILD-003 and WL-CRIT-006
  production-evidence responsibilities.

### WL-REL-005 - Publish and promote the newest complete release

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: version 13.0.0 has no immutable current tag or complete public
  asset set, and partial candidates must never enter the package channel.
- Required outcome: publish one immutable tag and GitHub release, verify all
  assets by readback, then atomically expose only signed package metadata.
- Current state: tags end at `magic-mesh-v12.1.1`. `WL-REL-004` has no signed
  six-role bundle, so publication is correctly refused. Never substitute an
  unsigned or promotion-forbidden preview.
- Remaining work: do not start until `WL-REL-004` S5 is green. Then:
  1. S1 Reconfirm publication authority and remote state.
  2. S2 Create and push signed annotated tag `magic-mesh-v13.0.0`.
  3. S3 Publish the GitHub release and exact assets.
  4. S4 Verify downloaded bytes independently.
  5. S5 Promote signed package metadata atomically with `repo_gpgcheck=1`.
  6. S6 Hand the exact identities to `WL-TEST-003` and flip that epic to
     Remaining only after a testing Beta exists.
- Scope: tag, GitHub release, asset readback, signed package metadata
  promotion, and acceptance handoff.
- Relevant files/components: Git remote/tag tooling, GitHub release workflow,
  verify-github-release-binding.sh, packaging/repo, dnf-channel helpers,
  release notes, and WL-TEST-003.
- Dependencies: blocked on `WL-REL-004`. Failed gate: no signed six-role
  bundle for `42035dcbd`.
- Acceptance criteria: tag, release, assets, signatures, provenance, and
  package metadata agree exactly; no partial release is visible.
- Verification method: after Status becomes Remaining, remote tag/release
  readback, clean-room asset verification, and HOLD/partial promotion refusal.
  Unblock payload: `@farm:{cargo test -p mde-enroll}`.
- Origin or merged source IDs: archived WL-BUILD-001, WL-BUILD-003, and
  WL-CRIT-006 publication responsibilities.

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
- Current state: Operator 2026-08-27 moved those leftovers here. Freeze-SHA
  workstation installs on Dell, Seat 15, and Surface (`42035dcbd` /
  `13.0.0-35`) and dest-cut `bc14a22d7` are not a testing Beta. This epic
  stays Awaiting testing until a testing Beta is released. leftover-units
  ignores Awaiting testing, so drain must not fan live-seat now. Operator
  2026-08-28 skipped Construct Health Fix until that Test Release exists.
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
  candidate, not a freeze-SHA seat install). Then the three-seat `13.0.0`
  qualification topology.
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

Feature-completion source is archived. `WL-FUNC-023` closed 2026-08-30.
`WL-FUNC-024` through `WL-FUNC-032` closed 2026-08-29. Live leftovers from
those epics stay on `WL-TEST-003`. Do not reopen a FUNC epic to fill the farm.

## Stewardship

Heading: `### WL-<FAMILY>-<NNN> - <title>`. Required fields in order:
Status, Priority, Complexity, Problem, Required outcome, Current state,
Remaining work, Scope, Relevant files/components, Acceptance criteria,
Verification method, Origin or merged source IDs. Optional: Dependencies.

Status is exactly `Remaining` / `Blocked` / `Awaiting testing` /
`Needs clarification`. Only Remaining epics emit farm jobs. Close by moving
the epic to `docs/worklist-archive/` with a disposition. Never reuse an ID.
Run `install-helpers/lint-worklist.sh` before landing worklist edits.
