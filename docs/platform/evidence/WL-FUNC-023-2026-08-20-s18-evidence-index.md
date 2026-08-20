# WL-FUNC-023 S18 evidence index — 2026-08-20

This index is the S18 handoff for the unified ONBOARD & OFFBOARDING
lifecycle. It records what the existing farm evidence actually proves and
what remains open. It does not promote fixture or farm-contract results into
installed-release or live-seat acceptance.

## Authority and evidence boundary

- Worklist source: `docs/platform/WORKLIST.md`, `WL-FUNC-023` S1–S18.
- Governance source: `AI_GOVERNANCE.md` §7 and §10. Static correctness is not
  production evidence; farm jobs are the heavy execution backend, and every
  farm job must have an admission record binding the immutable job, inputs,
  host, slot, capacity, and checkpoint evidence.
- Build source: `docs/BUILD-ENVIRONMENT.md`. The canonical farm is
  `172.20.0.50`, `.90`, `.130` (BigBoy), `.170`, and `.196`; the longest job
  belongs on BigBoy. The local Rocky host is not a heavy build/test lane.
- Acceptance boundary: exact installed-release and live acceptance is owned by
  `WL-TEST-002`. For `13.0.0`, deep physical acceptance is exactly Dell, Seat
  15, and Surface, plus the three independently required lighthouses. Eagle
  and T480 are non-gating inspection/deployment-wave seats.
- Retention boundary: detailed logs, Bus history, transfer ledgers,
  collaboration history, application history, and audit history have a
  six-hour maximum lifetime. Current state, identities, credentials, queued
  payloads, media, and VM disks are not history and must survive the epoch.

## Evidence already proven on the farm

The following are real farm results recorded in the cited evidence. They are
implementation, contract, hostile-fixture, or package-structure evidence only.

| Stories | Proven result | Evidence |
| --- | --- | --- |
| S1, S9, S13, S17 | First-boot audit refuses a planted missing unit; compute/hardware failures remain warnings; healthy convergence stamps only after the baseline; failed capsule staging is retained; stale status/touch shortcuts are rejected. `5 passed, 0 failed` on BigBoy. | [`WL-FUNC-023-2026-08-20-firstboot-baseline-farm-r1.md`](WL-FUNC-023-2026-08-20-firstboot-baseline-farm-r1.md) |
| S1, S3, S5, S6, S7, S8, S9, S10, S14, S16 | Lifecycle authority tests cover target/generation binding, exclusive locks, atomic checkpoints, interruption/resume, correction planning, readiness warnings, artifact admission, capsule retry/revocation, destructive confirmation scope, truthful fleet reports, and offboarding receipt completion. The current focused authority result is `17 passed, 0 failed`. | [`WL-FUNC-023-2026-08-16-lifecycle-authority-farm-r2.md`](WL-FUNC-023-2026-08-16-lifecycle-authority-farm-r2.md) |
| S2, S5, S6, S7, S8, S10, S14 | Typed lifecycle contract coverage rejects invalid versions, scope, transitions, replay, unbound artifacts, implicit unsigned admission, and rollback correction. The later contract result is `21 passed, 0 failed`; the earlier r1 record is superseded and is retained only as history. | [`WL-FUNC-023-2026-08-16-lifecycle-authority-farm-r2.md`](WL-FUNC-023-2026-08-16-lifecycle-authority-farm-r2.md), [`WL-FUNC-023-2026-08-16-lifecycle-authority-r1.md`](WL-FUNC-023-2026-08-16-lifecycle-authority-r1.md) |
| S3, S4, S5, S6, S7, S9, S10, S11, S14, S16 | The focused `mackesd onboard` suite covers first-desktop, invite/join, mesh creation/DNS/network, role provisioning, self-test, service-add, lighthouse spawning, remote push, and worker authorization/replay/retry/recovery. The recorded result is `231 passed, 0 failed`. | [`WL-FUNC-023-2026-08-20-firstboot-baseline-farm-r1.md`](WL-FUNC-023-2026-08-20-firstboot-baseline-farm-r1.md), [`WL-FUNC-023-2026-08-16-lifecycle-authority-farm-r2.md`](WL-FUNC-023-2026-08-16-lifecycle-authority-farm-r2.md) |
| S4 | Renderer-neutral lifecycle projection and `magic-setup` state-machine coverage passed (`34 passed, 0 failed`); canonical GUI/TUI plan projection passed (`1 passed, 0 failed`); the packaged Construct route reaches `/usr/bin/magic-setup` (`3 passed, 0 failed`). | [`WL-FUNC-023-2026-08-16-lifecycle-authority-farm-r2.md`](WL-FUNC-023-2026-08-16-lifecycle-authority-farm-r2.md), [`WL-FUNC-023-2026-08-16-construct-route-r1.md`](WL-FUNC-023-2026-08-16-construct-route-r1.md) |
| S6, S16 | Remote-push tests cover typed ordering/refusal, redaction, target separation, signed-bundle freshness, signer checks, nonce replay, thin-lighthouse policy, local application, and injected Bus/SSH seams. The current follow-up result is `26 passed, 0 failed`; missing-bearer refusal and private bearer handoff are separately covered. | [`WL-FUNC-023-2026-08-16-remote-push-farm-r1.md`](WL-FUNC-023-2026-08-16-remote-push-farm-r1.md), [`WL-FUNC-023-2026-08-16-bearer-handoff-farm-r1.md`](WL-FUNC-023-2026-08-16-bearer-handoff-farm-r1.md) |
| S11, S17 | Package/first-boot structural checks passed, including service activation and hostile RPM/bootc ordering/status fixtures. Role provisioning passed `24` tests and onboard self-test passed `29` tests on the farm. | [`WL-FUNC-023-2026-08-20-firstboot-baseline-farm-r1.md`](WL-FUNC-023-2026-08-20-firstboot-baseline-farm-r1.md), [`WL-FUNC-023-2026-08-16-lifecycle-authority-farm-r2.md`](WL-FUNC-023-2026-08-16-lifecycle-authority-farm-r2.md) |
| S14 | Local offboard execution and verification passed `5` focused tests; the completed receipt cannot retain reusable resources. This proves the local contract/executor boundary, not fleet drain or live target erasure. | [`WL-FUNC-023-2026-08-16-lifecycle-authority-farm-r2.md`](WL-FUNC-023-2026-08-16-lifecycle-authority-farm-r2.md) |

The cited records also contain successful daemon checks, a deterministic
serial `mackesd` library gate (`5021 passed; 0 failed; 1 ignored`), and focused
recovery/package checks. Those results are useful regression evidence, but they
do not independently close a story whose required package, provider, fleet, or
live evidence is absent.

## S18 implementation follow-up — 2026-08-20

The lifecycle-specific implementation changed the production
`LiveProvisioner::push_enroll` path. When a cloud/provider endpoint does not
return a bearer, the configured lifecycle workgroup root now mints one through
the existing bearer ledger with `role:lighthouse` scope and hands it to the
typed `SshBootstrap` action. The command-template placeholder is never used as
a credential; a missing root remains a typed integration gate. Provider-issued
bearers remain supported, and a failed handoff leaves a newly issued ledger
entry pending for corrected-forward retry.

Farm admission and results:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=3 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  onboard::spawn_lighthouse -- --nocapture
```

- BigBoy `.130`, slot `3`; admission reported 38,881,316 KiB free.
- Source synced to the admitted farm checkout; `15 passed, 0 failed, 5011
  filtered out` (5m37s total command time).
- The new `push_enroll_mints_a_scoped_bearer_when_provider_did_not_return_one`
  test proves ledger pending state, lighthouse scope, placeholder refusal, and
  redacted action output.

```text
MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  onboard::remote_push -- --nocapture
```

- `.196`, slot `1`; admission reported 30,102,288 KiB free.
- `26 passed, 0 failed, 5000 filtered out` (4m01s total command time).
- Existing typed SSH/Bus refusal, redaction, nonce, signature, and
  acknowledgement coverage remains green.

These are farm implementation/contract results only. They do not claim that a
real cloud provider returned an endpoint, that live SSH enrollment succeeded,
or that the exact candidate was installed on Dell, Seat 15, or Surface.

## Story disposition at S18

| Story | Disposition | Exact boundary |
| --- | --- | --- |
| S1 | Farm contract evidence present; integration baseline not fully demonstrated. | The canonical readiness and first-boot checks are tested, but a complete installed baseline remains in WL-TEST-002. |
| S2 | Farm typed-contract evidence present. | No live or installed claim. |
| S3 | Farm authority/recovery evidence present. | No proof of every real provider-side executor effect. |
| S4 | Partial farm evidence. | Projection, TUI state machine, and route reachability are tested; full Construct GUI/TUI parity on an installed seat remains open. |
| S5 | Farm hostile authorization evidence present. | No physical seat mutation or live trust/provider proof. |
| S6 | Farm capsule, scoped-bearer minting, redaction, refusal, and handoff evidence present. | Live SSH enrollment and provider-side execution remain open. |
| S7 | Farm exact artifact-selection contract evidence present. | Candidate-bound release inputs and installed artifact identity belong to the release chain/WL-TEST-002. |
| S8 | Farm unsigned-artifact confirmation evidence present. | No installed-release qualification claim. |
| S9 | Focused missing-input/readiness evidence present. | Full provider, hardware, compute, storage, and installed-seat discovery remains open. |
| S10 | Farm correction-plan and resume evidence present. | Live corrected-forward repair and recovery drills remain WL-TEST-002. |
| S11 | Farm package/first-boot structure and fixtures present. | Clean RPM, bootc, Kickstart/NoCloud, USB, and no-manual-step installed proof remains open. |
| S12 | Farm artifact binding and upgrade safety boundaries present. | Turnkey installed upgrade, migration, reboot, and pending-convergence proof remains open. |
| S13 | Farm warning classification present. | Dell/Seat 15/Surface hardware and capability withdrawal must be proved live. |
| S14 | Local farm offboard boundary present. | Fleet drain, revoke, placement verification, remote execution, and physical erase remain open. |
| S15 | Typed ResetAndOnboard plan projection is covered. | Full wipe, replacement identity, reinstall, and re-enrollment are not proven. |
| S16 | Farm fleet-report and transport seams are covered. | Persistent live fleet execution, coordinator handoff, reconnect, and mixed-state recovery are not proven. |
| S17 | Current first-boot farm evidence is present. | Installed RPM/bootc first boot and exact candidate identity remain open. |
| S18 | This index is prepared; implementation closure is not asserted. | The remaining farm gates and the exact WL-TEST-002 handoff must be resolved before archive/closure. |

## Remaining WL-FUNC-023 implementation gaps

These are not silently transferred to WL-TEST-002:

1. Complete GUI/TUI convergence over the shared lifecycle session and ensure
   legacy lifecycle routes contain no renderer-owned business logic.
2. Complete the real package/service/enrollment executor path behind the tested
   authority, including turnkey RPM, bootc, Kickstart/NoCloud, USB, and
   first-boot behavior.
3. Complete installed-capable upgrade, VerifyAndCorrect, ResetAndOnboard,
   fleet offboard, and coordinator-handoff execution paths; the existing farm
   tests prove boundaries and seams, not every side effect.
4. Produce fresh focused farm evidence for any implementation changes above,
   with explicit `MCNF_BUILD_HOST` and `MCNF_BUILD_SLOT`, an admitted job
   identity, result, source revision, and artifact/checkpoint record. The
   longest job goes to BigBoy; no filler or duplicate job is evidence.

## Exact deferred WL-TEST-002 obligations

After a clean, exact, signed candidate and admitted release inputs exist,
`WL-TEST-002` owns the following:

1. Admit the immutable candidate and record pre-mutation hardware, package,
   authorization, and corrected-forward recovery baselines.
2. On exactly Dell, Seat 15, and Surface, emit the red
   `AI-GENERATED-ALERT`, wait five seconds, install the exact bytes, reboot
   only when required, and prove installed identity, services, shell/About/
   watermark/welcome/mesh-help versions, and honest degraded states.
3. Prove authorized providers and collaboration paths, including calls,
   mute/consent/revocation/reconnect, chat/alerts, files/transfers, editor,
   and clipboard. Missing providers must remain visibly unavailable.
4. Capture direct-DRM Construct acceptance with route identity, dimensions,
   hashes, and machine-verifier results.
5. Prove media and physical integrations, including audio/video, Cast, DLNA,
   loss, typed handoff, and recovery with real devices/providers.
6. Prove Browser VM, App VM, and bootc guest/device roles against signed
   artifacts, including input/audio/GPU/reconnect/failure behavior; Android is
   deferred and is not a gate.
7. Run corrected-forward resilience drills: process/session, lock/sleep,
   network/storage loss, generation change, reboot, and re-enrollment, while
   proving six-hour history retention/expiry.
8. Reconcile every result, reopen the exact owning implementation or
   infrastructure story on failure, and produce the signed acceptance index.

Three lighthouses retain independent topology/quorum proof. Eagle and T480
must not be counted as additional deep-acceptance seats. A broader preview
distribution, if explicitly authorized, is manifest-bound and promotion-
forbidden; it does not satisfy any of the obligations above.

## Current blockers and non-claims

- `WL-TEST-002` is currently `Blocked`: the exact current-source signed
  six-role candidate and admitted production-input receipts are not yet
  available for qualification.
- The worktree used by the cited 2026-08-20 evidence is dirty. The recorded
  farm results therefore cannot be treated as proof of the future clean,
  pushed release candidate.
- The evidence files record farm hosts, slots, commands, and results, but this
  index does not invent missing farm admission records, artifact digests,
  GitHub required-check receipts, SBOM/provenance, installed identities,
  provider credentials, live Bus acknowledgements, live SSH results, or
  physical-seat observations.
- Existing evidence explicitly leaves live token minting, live SSH bootstrap,
  live provider effects, package integration, fleet handoff, installed
  first-boot, and physical-seat acceptance open. Those gaps remain open here.
- Production promotion is blocked while required live, installed, recovery,
  provenance, SBOM, and required-check evidence is missing. No fixture,
  compilation result, historical evidence, manual assertion, or wider preview
  distribution closes that block.

No `docs/platform/runbooks/` directory exists in this worktree; accordingly,
no runbook file was created outside the permitted owned paths.
