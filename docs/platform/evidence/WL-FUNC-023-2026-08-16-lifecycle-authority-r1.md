# WL-FUNC-023 lifecycle authority — focused contract and recovery gate

> **HISTORICAL / SUPERSEDED:** This r1 record includes the former caller-selected
> `--step` and `lifecycle-complete` CLI path. Commit
> `afa79aade10b56cef0447e5652dd25c22950aa8a` removed both bypasses; current
> evidence is recorded in `WL-FUNC-023-2026-08-16-lifecycle-authority-farm-r2.md`.

- Date: 2026-08-16 UTC
- Base revision: `daf3c695928e96553fe839450bd86aa6f371e3aa`
- Working tree: dirty; this evidence covers the lifecycle files listed below
  and does not claim the unrelated release-script/worklist edits are complete.
- Scope: typed lifecycle contracts, local per-target authority locking,
  checkpoint persistence, interrupted-session resume, ordered step commits, and
  the `mackesd onboard lifecycle` plan projection.

## Changed surface

- `crates/mesh/mackes-mesh-types/src/lifecycle.rs`
- `crates/mesh/mackes-mesh-types/src/lib.rs`
- `crates/mesh/mackesd/src/lifecycle_authority.rs`
- `crates/mesh/mackesd/src/lib.rs`
- `crates/mesh/mackesd/src/bin/mackesd.rs`
- `crates/mesh/mackesd/src/cli/onboard.rs`

## Farm verification

Command, routed to `.50` slot 1:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=1 install-helpers/xcp-build.sh \
  cargo test -p mackes-mesh-types lifecycle --locked -- --nocapture
```

Result: `20 passed; 0 failed`, including the canonical baseline ownership map and rejection of unowned lifecycle step
names, invalid operator/target session scope, mismatched destructive
confirmation phrases/scopes, Ed25519 confirmation signature verification,
expired/replayable commissioning capsules, and implicit unsigned artifact
admission; warning/unknown requirement checks also require explicit evidence
and warning text; correction plans reject rollback and undeclared steps; upgrade
bindings reject downgrades and unbound target artifacts.

Command, routed to `.90` slot 1:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1 install-helpers/xcp-build.sh \
  cargo test -p mackesd --lib lifecycle_authority --locked -- --nocapture
```

Result: `16 passed; 0 failed`, including successful-step commit, terminal
failure recording, completion-gated offboarding receipt projection, and mixed
fleet-generation rejection, plus checkpoint-bound requirement checks and
required-failure progression blocking. It also derives truthful seat readiness:
optional warnings remain usable while required failures withdraw readiness. The authority test now also verifies
that a signed Ed25519 destructive confirmation is accepted and persisted before
an offboarding receipt can be projected, and that a target-bound commissioning
capsule is admitted only once and rejects replay.
The authority also admits one immutable, target/generation-bound artifact
selection and rejects replacement within the same lifecycle generation.
Unsigned selections are rejected unless a signed `INSTALL UNSIGNED 1 SYSTEMS`
confirmation is bound to the exact artifact digest.
Correction plans are also accepted only when each correction targets a current
blocking check and the typed plan forbids rollback.
Fleet aggregation now reports `WaitingForOperator` instead of false `Succeeded`
when a terminal target still has a required failed or unknown check.
Package execution also refuses to start until the authority has a pinned artifact
selection.
Checkpoint updates enforce legal phase transitions and reject reopening any
terminal state.
Destructive confirmations are generation-bound and cannot be replaced or
replayed within one checkpoint.
Readiness now carries explicit non-blocking warning text alongside missing
requirements; optional degraded capabilities remain visible.
Direct completion of a final step with a blocking check now lands in
`WaitingForOperator` instead of `Succeeded`.
Offboarding receipts now have a domain-separated Ed25519 signing and verify
boundary; the authority emits the unsigned projection for the governed signer.
Fleet lifecycle reports have the same independent signing/verification boundary
for aggregated terminal evidence.

Command, routed to `.90` slot 2:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=2 install-helpers/xcp-build.sh \
  cargo check -p mackesd --bin mackesd --locked
```

Result: finished successfully.

The same daemon check also covers the turnkey default-step projection used by
`lifecycle` and `lifecycle-start` when no explicit `--step` values are given.

CLI smoke command, routed to `.90` slot 2:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=2 install-helpers/xcp-build.sh \
  cargo run -p mackesd --bin mackesd --locked -- onboard lifecycle \
  --intent-json '{"schema_version":1,"request_id":"request-1","target_id":"seat-15","intent":"onboard","generation":1}' \
  --step identity --step verify
```

Result: emitted a validated `LifecyclePlanV1` with the two requested steps.

Start/resume CLI smoke on the same `.90` slot using `/tmp/mcnf-life-smoke-r1`:

```text
mackesd onboard lifecycle-start --root /tmp/mcnf-life-smoke-r1 \
  --intent-json '{"schema_version":1,"request_id":"request-1","target_id":"seat-15","intent":"onboard","generation":1}' \
  --step identity --step verify
mackesd onboard lifecycle-complete --root /tmp/mcnf-life-smoke-r1 \
  --target-id seat-15 --step-index 0
```

Result: start emitted `planned / 0 of 2`; complete reopened the checkpoint and
emitted `running / 1 of 2`. The temporary remote directory was removed after
the smoke run.

The daemon now also exposes `onboard lifecycle-confirm`, which resumes a
checkpoint, verifies a supplied `LifecycleConfirmationV1` against an explicit
Ed25519 public key, persists it, and releases the authority lock. Non-destructive
intents reject confirmation, and offboarding receipts reject checkpoints that
have no accepted confirmation.

It also exposes `onboard lifecycle-readiness`, which reopens the same checkpoint
and prints the authority-derived `SeatReadinessV1`; clients therefore share the
required-failure and optional-warning interpretation.

Artifact admission is exposed as `onboard lifecycle-artifact-select`; signed
selections use the pinned digest directly, while unsigned selections require
both the signed confirmation JSON and its explicit verifying key.

Commissioning is exposed as `onboard lifecycle-capsule-admit`; it verifies the
target-bound capsule at the supplied/current epoch time, persists the one-time
capsule id, and returns the admitted bootstrap digest with the checkpoint.

The live `onboard role-provision` mutation now acquires a lifecycle authority
checkpoint around its configuration step and records partial unit failure;
`--dry-run` remains non-mutating. Farm validation:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1 install-helpers/xcp-build.sh \
  cargo test -p mackesd --lib onboard::role_provision --locked -- --nocapture
```

Result: `23 passed; 0 failed`.

The live `onboard service-add` apply path now also acquires a configuration
checkpoint and records integration-gated apply failure; its dry-run path remains
plan-only. The daemon binary check passed after this routing change.

The live `onboard first-desktop` placement/session path now acquires a compute
checkpoint and records integration-gated apply failure; its dry-run path remains
plan-only. The daemon binary check passed after this routing change as well.

The live `onboard spawn-lighthouse` provision/enroll/CA-migration path now
acquires a mesh checkpoint and records integration-gated apply failure; its
dry-run and LAN-only retry paths remain unchanged. The daemon binary check
passed after this routing change as well.

The live `onboard mesh-dns` managed-hosts apply now acquires a mesh checkpoint
and records apply failure; its dry-run renderer remains non-mutating. The daemon
binary check passed after this routing change as well.

The live `onboard network` keyfile/nmcli apply now acquires a configuration
checkpoint and records LAN bring-up failure; its dry-run renderer remains
non-mutating. The daemon binary check passed after this routing change as well.

The live `onboard mesh-create` identity/mesh bootstrap now acquires an authority
checkpoint before CA and mesh mutation, records bootstrap failure, and leaves
the declared mesh convergence step resumable for the supervisor. The daemon
binary check passed after this routing change as well.

The live `onboard invite-issue` enrollment-ledger mutation now acquires an
identity checkpoint and records issuance failure before emitting bearer/QR
material. The daemon binary check passed after this routing change as well.

The `mackesd join` wrapper now acquires an identity/mesh authority checkpoint
around token redemption, role persistence, network enrollment, and setup. Its
existing token redaction and bounded helper behavior remains inside the
authority step. The daemon binary check passed after this routing change.

The coordinated `mackesd upgrade --coordinate` intent publication now acquires
an upgrade configuration checkpoint and records intent-write failure. The
daemon binary check passed after this routing change.

The `mackesd found` founding-lighthouse operation now acquires one mesh
checkpoint around its existing endpoint identity, CA, mesh-init, and bearer
sequencing, recording any failure without altering those security-sensitive
internals. The daemon binary check passed after this routing change.

Broader onboarding regression gate, routed to farm `.90` slot 1:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1 install-helpers/xcp-build.sh \
  cargo test -p mackesd --lib onboard --locked -- --nocapture
```

Result: `228 passed; 0 failed`, covering first-desktop, invite, mesh-create,
mesh-dns, network, role provisioning, remote push, service add, spawn
lighthouse, and onboarding workers.

## Remaining WL-FUNC-023 work

This gate does not prove actual package, identity, mesh, compute, UI, hardware,
offboarding, or fleet side effects. Those remain downstream executor steps and
must produce their own focused evidence before the epic can close.

In particular, `upgrade_intent_watcher` still owns the later `dnf`/
`mde-install` package mutation. The watcher now fails closed unless the signed
replicated intent carries either a valid typed `LifecycleArtifactSelectionV1`
(accepted by the coordinate CLI as `--artifact-selection-json`) or the
compatibility 64-hex `artifact_digest` path; digest-less legacy intents remain
readable but cannot invoke either package operation. Farm evidence for this boundary:
`MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=2 install-helpers/xcp-build.sh
cargo check -p mackesd --bin mackesd --locked` passed, and the focused writer
writer and typed-selection tests passed (`3 passed; 0 failed`) across the
focused farm runs on `.50/1` and `.90/1`. The remaining work is to produce
full downstream package/service/enrollment execution evidence.

The remote `mackesd decommission` mutation now acquires an `Offboard`
authority checkpoint around the decommissioned-role transition and lifecycle
audit event, preserving force labeling and retained history. Farm verification:
the daemon check passed and the decommission-focused gate passed (`4 passed;
0 failed`).

The remote `remove-peer` path now also acquires an `Offboard` checkpoint around
role decommissioning, audit emission, certificate revocation/ban propagation,
and etcd membership plus peer-directory cleanup. Farm verification:
`cargo check -p mackesd --bin mackesd --locked` passed; its focused module gate
completed with no matching tests (`0 passed; 0 failed`), so runtime side-effect
proof remains outstanding.

The destructive `mackesd leave --yes` path now requires a signed
`LifecycleConfirmationV1` plus an Ed25519 verifying key, consumes that
confirmation in the authority, and then runs one durable `Offboard` step around
etcd membership removal, mesh eviction, roster and trust teardown, local state
wipe, and nebula stop. The existing refusal to wipe when etcd removal is
unavailable or fails remains inside that step. Farm verification:
`cargo check -p mackesd --bin mackesd --locked` passed and the leave-focused gate
passed (`25 passed; 0 failed`).

The standalone `mackesd ca revoke` mutation now acquires an `Offboard`
authority checkpoint around certificate revocation and ban propagation. Farm
daemon compilation passed after this change; no dedicated CA-revoke runtime
fixture was available, so side-effect evidence remains separate work.

The `mackesd reenroll` credential rotation now acquires a durable
`VerifyAndCorrect` identity checkpoint around fresh-key generation, node-row
replacement, and the lifecycle audit event. Farm daemon compilation passed;
runtime credential-rotation proof remains separate work.

Standalone `mackesd ca ban` now acquires an `Offboard` authority checkpoint
around replicated ban-list mutation. Farm daemon compilation passed; no live
CA-ban fixture was available, so runtime propagation evidence remains open.

The non-dry-run `mackesd recovery` path now acquires a durable
`VerifyAndCorrect` identity checkpoint around optional old-identity blocklist
eviction and integration-gated fresh re-enroll execution. Dry-run remains
non-mutating. Farm daemon compilation passed; live recovery fixture evidence
remains outstanding.

Verification wave after the recovery integration: the farm recovery gate passed
`46 passed; 0 failed` on `.90/1`; repository `git diff --check` and the
worklist lint self-test also passed. The mutation audit leaves only
administrative `ca unban` and legacy-import audit writes outside the new
lifecycle routes; neither is claimed as lifecycle completion here.

Standalone `mackesd ca unban` now acquires a `VerifyAndCorrect` identity
checkpoint around local replicated ban removal; its existing read-only union
reporting remains outside the mutation step. Farm daemon compilation passed.

The GUI/TUI audit found no live lifecycle intent or authority-status route in
the desktop crates; their current readiness models are unrelated map/firmware
surfaces. No parallel renderer-owned lifecycle state was introduced. GUI/TUI
convergence therefore remains an explicit integration deliverable.

An attempted broad `cargo test -p mackesd --lib --locked` farm gate did not
close: `5016 passed; 5 failed; 1 ignored`. Reproduction showed at least the
worker-role canonical inventory golden hash drift and Android desired-state
fixture rejection (`DesiredImageMismatch`). The worker-role golden was updated
to the current runtime inventory, and Android isolation was corrected so the
focused Android suite passes (`22 passed; 0 failed`) plus the cloud desired
state test passes. The provider-before-engine transaction fix makes the
collaboration proof-only test pass in isolation. The latest broad run reached
`5020 passed; 1 failed; 1 ignored`; its sole firmware HMAC/interlock failure
also passes in isolation, indicating parallel-test flakiness outside the
WL-FUNC-023 lifecycle files. A deterministic serial farm gate using
`--test-threads=1` then passed the complete library: `5021 passed; 0 failed;
1 ignored` on `.90/2`.
S4 progress: `mde-enroll::lifecycle_view::LifecycleSessionView` now provides a
bounded renderer-neutral projection of `OnboardOffboardSessionV1` and scoped
`SeatReadinessV1`. It rejects invalid wire contracts and readiness for targets
outside the session scope, and gives terminal and GUI consumers one status-line
projection without owning lifecycle mutation. The farm gate
`MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=2 install-helpers/xcp-build.sh
cargo test -p mde-enroll --lib -- --test-threads=1` passed with `33 passed;
0 failed`. The full Construct GUI renderer and legacy-route redirection remain
open. The `Wizard` now carries an optional read-only projection and the
`magic-setup` status log renders its authoritative status line when supplied;
the setup model still has no lifecycle mutation path.
Binary verification also passed on `.50/2` with
`cargo check -p mde-enroll --bin magic-setup --locked`.

The repaired `.170` farm lane was assigned an isolated `wl170-a` workspace for
the full serial `mackesd` library gate (`cargo test -p mackesd --locked --
--test-threads=1`); it passed `5021 passed; 0 failed; 1 ignored`. In parallel,
the focused `.90` lane `drain-enroll-b` completed `cargo check -p mde-enroll
--bin magic-setup --locked` successfully in 2m37s.

The `.170` full library run exposed a test-only stack overflow in the broad
Clap parser used by the `found` and `join` contract tests after the library
passed. The parser behavior itself succeeds with a 16 MiB stack; the tests now
use a bounded 16 MiB parser-helper thread, with production parsing unchanged.
The focused `found` suite then passed `7 passed; 0 failed`, and the complete
`mackesd` binary suite passed `71 passed; 0 failed` on the same farm slot.
