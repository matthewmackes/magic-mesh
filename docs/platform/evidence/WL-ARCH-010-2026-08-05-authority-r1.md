# WL-ARCH-010 evidence — typed authority and Display1 seams (2026-08-05)

Working-tree base revision: `e52322ec` (changes are intentionally uncommitted).

## Farm gates

The following commands were executed through `install-helpers/xcp-build.sh`
with an explicit farm host and slot:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-compute-domainmap \
  cargo test -p mackesd --features async-services --lib workload_
  32 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-drm-cleanup \
  cargo test -p mde-egui --features drm \
    drm::tests::external_dmabuf_metadata_is_bounded_before_prime_import
  1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=arch010-shell-display1b \
  cargo test -p mde-shell-egui --features drm --tests display1_client::
  1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-libvirt-display1 \
  cargo test -p mackesd --features async-services --lib \
    workers::vm_lifecycle::tests::domain_xml_includes_virtio_gpu_accel3d
  1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-browser-migrate \
  cargo test -p mde-shell-egui --tests web::tests
  15 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-display-relay2 \
  cargo test -p mackesd --features async-services --lib display1_broker
  4 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-types-signals \
  cargo test -p mackes-mesh-types workloads::
  5 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=arch010-datacenter-typed-check \
  cargo check -p mde-shell-egui --tests
  finished successfully

MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=arch010-shell-display1c \
  cargo check -p mde-shell-egui --features drm --tests
  finished successfully

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-storage-geom
  cargo test -p mackesd --features async-services --lib workers::storage::
  49 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-first-desktop
  cargo test -p mackesd --features async-services --lib onboard::first_desktop::
  21 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-first-desktop-workload-rename
  cargo test -p mackesd --features async-services --lib onboard::first_desktop::
  21 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-journal-before-actuator
  cargo test -p mackesd --features async-services --lib workers::workload_compute::
  4 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-retire-legacy-registry-rerun3
  cargo test -p mackesd --features async-services --lib worker_role::
  24 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-profile-names
  cargo test -p mackes-mesh-types workloads::
  6 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=arch010-profile-callers
  cargo check -p mde-shell-egui --tests
  finished successfully

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-image-define-contract-check
  cargo check -p mackesd --features async-services --lib
  finished successfully

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-image-define-tests
  cargo test -p mackesd --features async-services --lib workers::workload_compute::
  5 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-image-contract-types
  cargo test -p mackes-mesh-types workloads::
  6 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=arch010-image-shell-client
  cargo test -p mde-shell-egui --tests workload_api::
  3 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-display1-drm-final
  cargo check -p mde-egui --features drm
  finished successfully

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-display1-shell-final
  cargo test -p mde-shell-egui --features drm --bin mde-shell-egui display1_client
  2 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-destroy-error-classification
  cargo test -p mackesd --features async-services --lib workers::workload_compute::
  6 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-storage-layout-rerun2
  cargo test -p mackesd --features async-services --lib workers::storage::
  49 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-strict-startattach-r2
  cargo test -p mackesd --features async-services workload_compute
  8 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-display-relay2
  cargo test -p mackesd --features async-services --lib display1_broker
  5 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-display1-path-r1
  cargo test -p mde-shell-egui --features drm --bin mde-shell-egui display1_client
  3 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-display-server-r1
  cargo test -p mackesd --features async-services --lib display1_broker
  6 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-display1-handshake-r1
  cargo test -p mde-shell-egui --features drm --bin mde-shell-egui display1_client
  blocked by farm fixture exhaustion: `No space left on device` (exit 101)

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-actuator-display1-r2
  cargo test -p mackesd --features async-services --lib workers::workload_compute::
  10 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-display1-handshake-r2
  cargo test -p mde-shell-egui --features drm --bin mde-shell-egui display1_client
  3 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-display-broker-timeout-r1
  cargo test -p mackesd --features async-services --lib display1_broker
  6 passed, 0 failed
```

The first chooser attempt on `172.20.0.50` reached 103 passing tests but its
linker failed with `Disk full?`; after cleaning that exact farm fixture, the
clean rerun on `172.20.0.90` completed **104 passed, 0 failed**. No live
Dell/seat-15 acceptance evidence has been claimed.

## Authority/lint gates

```text
install-helpers/lint-worklist.sh --self-test
install-helpers/lint-worklist.sh
install-helpers/lint-doc-supersession.sh
install-helpers/lint-workload-authority.sh
git diff --check
```

All passed. The authority lint rejects legacy VM/container lifecycle topics
from production shell sources; Fleet Start/Stop now use the typed Workload
operation lane, and Fleet Create redirects to the Workloads stepper. The lint
is now part of the maintained `ci-gate.sh policy` suite and has its own
fail-closed self-test; the farm fallback path was exercised on a node without
`rg`.

## Authority-guard integration slice (2026-08-05)

The bounded WL-ARCH-010 execution slice adds `--self-test` coverage and a
portable `rg`/`grep` scan to `install-helpers/lint-workload-authority.sh`.
The guard records the migration map
`action/vm/lifecycle`/`action/container/lifecycle` →
`action/workload/operation`, rejects those publishers in production shell
sources, requires the canonical `workload_compute` spawn, and rejects a
retired VM/container actuator in the production spawn registry. It is wired
into both policy-lint and policy-self-test lists in `install-helpers/ci-gate.sh`.

Farm verification used explicit isolated slots:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-authority-lint
  lint-workload-authority.sh --self-test + real-tree scan: passed
  ci-gate.sh --self-test: passed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-workload-guard
  cargo test -p mackesd --features async-services --lib workers::workload_compute::
  11 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=arch010-worker-census
  cargo test -p mackesd --features async-services --lib worker_role::tests::
  24 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-policy2
  ci-gate.sh policy: failed only at the pre-existing style-leak lint; the
  five reported raw hover-text sites are in chooser/resources.rs,
  iac/android_apps.rs, web/mod.rs, and status_bar.rs and are outside this
  authority slice. Worklist, supersession, authority, bus-name, tier,
  brand, and shared-substrate policy checks passed.
```

The source tree remains intentionally uncommitted and the full-tree format
gate remains non-clean from unrelated edits. This slice proves the new
authority regression guard and focused Workload behavior; it does not claim
completion of Display1 active-loop integration, remaining caller migration,
live Dell/seat-15 proof, or the broader policy aggregate.

The full-tree farm rustfmt gate remains non-clean because the shared working
tree contains unrelated pre-existing edits; the targeted first-desktop file
also has pending rustfmt output. The RDP connect module is present and builds
in the release cut. This is recorded as a formatting blocker, not a completion
claim.

## Remaining proof

The Display1 source is now non-blocking and is consumed by the active DRM loop:
the loop imports bounded DMA-BUF metadata directly into KMS, retains the FD
through page-flip completion, preserves the latest native frame on idle, and
tears down in order before resuming GBM on disconnect. Full Display1 damage,
cursor, input, audio, clipboard, and resize integration, pool
mount/subtree/SELinux application, remaining daemon onboarding caller
migration, and live Dell/seat-15 plus fleet acceptance remain Remaining under
the epic. The storage worker now has a hostile-tested contiguous-extent
Workstation XFS preview, bounded parted/xfsprogs geometry executor, and an
exact post-create mount/container-subtree/SELinux layout action; this is not a
claim that a live pool was created. No compile-only gate is treated as
completion.

The strict StartAndAttach farm gate closes the prior false-success path: the
compute worker no longer converts a merely running libvirt domain into
`Completed` for an attach request. Completion now requires an
adapter-supplied, generation-bound attachment lease and `Ready` after a real
first frame; a premature completion is journaled as an actionable failure.
The production adapter remains in Display1/first-frame phases until the real
broker and KMS path report that frame. This is a correctness hardening result,
not evidence that live broker, first-frame, or performance acceptance is
complete.

The daemon now has a real node-local attachment-server lifecycle in addition to
the deterministic endpoint contract: it binds the exact lease socket, validates
the versioned lease handshake and kernel peer credentials, rejects nonce replay,
and exposes first-frame state only after a bounded SCM_RIGHTS relay accepts a
frame. The focused daemon gate passes 6/6, and the matching shell handshake gate
passes 3/3 after the exact farm fixture was cleaned. The earlier `r1` attempt
was blocked by the farm fixture running out of space before compilation; it is
retained as an environment incident, not a code result.

The daemon and shell now share a deterministic node-local endpoint contract:
`/run/mde/display1/<lease_id>.sock`. The shell may still receive an explicitly
provided socket path, but if it is absent it derives this path from the
validated, path-safe lease instead of silently abandoning native attachment.
Invalid path components are rejected. The focused broker and shell gates cover
 this mapping alongside existing SO_PEERCRED, nonce-replay, and nonblocking
DMA-BUF tests. A second shell gate (`arch010-display1-dynamic-r4`) passes 4/4,
including the bounded background worker that discovers the newest typed
generation-bound lease without doing persistence or socket I/O on the render
thread. QEMU listener registration, KMS/EGL
attachment, and live Dell/seat-15 proof are still required before this becomes
runtime proof.

The production Workload actuator now owns that server lifecycle: it creates a
deterministic generation-bound lease before defining/starting a VM, persists the
lease in the typed operation status, recreates the server from persisted status
after daemon restart, probes libvirt for the QEMU DBus graphics endpoint, and
keeps the registered Display1 peer alive until cleanup. Its focused farm gate
(`arch010-actuator-display1-r4`) passes 11/11, including deterministic
generation/lease tests, QEMU endpoint normalization, server reuse and teardown.
The gate does not replace live QEMU registration or first-frame proof.

The same gate proves unknown CPU capacity is denied before any adapter side
effect. The production probe now uses zero (unknown) rather than inventing a
one-thread host when `available_parallelism` cannot answer; the existing
mandatory CPU/memory/storage reserve then fails admission closed.

## Durable observation backoff (2026-08-06)

`WorkloadComputeWorker::reconcile_inflight` now honors the journaled
`next_retry_at_ms` for post-admission adapter observations as well as for the
queued/defining path. Before this guard, a transient `observe` error could be
re-issued on every poll tick despite the bounded retry budget, creating a
restart/recovery storm. Deadline expiry remains checked first, so an expired
operation still fails closed immediately. The hostile regression test models
the durable `Queued -> WaitingForGuest` boundary, injects a retryable observe
failure, proves an early recovery poll performs no second observation, and
proves the next retry deadline does.

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-retry-backoff-r2-big \
  CARGO_INCREMENTAL=0 install-helpers/xcp-build.sh \
  cargo test -p mackesd --features async-services --lib \
  workers::workload_compute
result: 18 passed, 0 failed; 4,383 filtered out
```

The full `mackesd` package formatter remains non-clean from unrelated dirty
tree edits; the new regression's formatter-only difference was corrected and
the focused executor gate is clean. This slice does not claim live adapter,
restart/crash recovery, Display1/KMS, Dell, or seat-15 acceptance.

## Expired attachment lease recovery (2026-08-06)

The typed projection intentionally removes an expired Display1 lease without
mutating the durable operation. The production adapter now treats that
persisted expiry as recoverable: it skips the stale descriptor and recreates a
fresh lease for the same workload generation. A hostile test starts the
node-local broker from an expired persisted status and verifies that the new
lease is valid, live beyond the recovery instant, and not the expired record.

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-retry-backoff-r2-big \
  CARGO_INCREMENTAL=0 install-helpers/xcp-build.sh \
  cargo test -p mackesd --features async-services --lib \
  workers::workload_compute
result: 19 passed, 0 failed; 4,383 filtered out
```

This proves the lease-rehydration seam only; live QEMU registration, KMS/EGL
first-frame proof, backend restart/crash recovery, Dell, and seat-15 acceptance
remain open.

## Live reachability snapshot

Read-only SSH checks succeeded against both required first hosts:

```text
172.20.0.15   Basement-Test-Workstation  Fedora 7.1.4-202.fc44  mackesd=active  shell=active  /dev/dri/card1
172.20.146.225 DELL-LAPTOP                Fedora 7.1.4-204.fc44  mackesd=active  shell=active  /dev/dri/card1
```

The libvirt query was blocked by the hosts' missing polkit agent for
`org.libvirt.unix.manage`, so no VM inventory is claimed. Both hosts report the
unrelated failed `fwupd-refresh.service`; loads were `1.74/1.80/1.79` (seat 15)
and `3.01/3.20/3.45` (Dell), with four logical CPUs and about 15.8 GiB RAM.
This is reachability/diagnostic evidence only, not native-frame, input, or
performance acceptance proof.

## Corrected-forward Dell deployment (2026-08-05)

The current working tree was cut on BigBoy through the Fedora 44 container lane
after the release was advanced above Dell's installed review package:

```text
artifact: magic-mesh-12.1.6-4.x86_64.rpm
size:     87,086,383 bytes (83.1 MiB)
sha256:   5a4fcb56931425cf065a23b2cc0199eba2cbbb0db132e82838872d8cd0d74d2f
farm:     172.20.0.130 / arch010-dell-current-f44
payload:  verify-rpm-payload.sh payload — all checks passed
```

Dell `172.20.146.225` accepted the mandatory visible seat warning, matched the
artifact checksum, passed `rpm -Uvh --test`, and installed the package without
a reboot. The pre-install state was `magic-mesh-12.1.6-3`, with both services
active and zero restarts. After the install, daemon reload, and ordered service
restarts, the exact live state was:

```text
rpm:      magic-mesh-12.1.6-4.x86_64
mackesd:  active/running, MainPID 3826109, NRestarts=0
shell:    active/running, MainPID 3828855, NRestarts=0
mackesd:  aa6ad5ff3f8005b558264bb99f8018b216bf3dd26948e0774d46f7ba4a7c4278
shell:    af555c729a6dd5d3c0ac3276c600b7ee5af893c6963cf658d914f39e37b6da30
version:  12.1.6 "Construct" · nogit · 2026-08-05 · dev
rpm -V:   clean
```

The temporary Dell RPM was removed after verification. This proves corrected-
forward installation and service health only; it does not waive the remaining
native Display1, input/performance, workload recovery, seat-15, or fleet
acceptance gates above.

## Dell boot-delay diagnosis and fix slice (2026-08-05)

Read-only boot tracing against `DELL-LAPTOP` (`172.20.146.225`) identified two
serial restart hazards in the installed 12.1.6-4 units:

```text
mcnf-mesh-secret-recipient.service: 20.076s → 65.230s (45s timeout)
mackesd.service:                  started at 66.536s
mde-shell-egui.service:           started at 72.286s; desktop handoff 93.218s
mcnf-cloud-arm-credential:        materialized at 75.915s
mackesd restart after credential:  78.689s → 168.725s (90s stop timeout)
```

The recipient reconciler was ordered `Before=mackesd.service` and `mackesd`
had a matching `After=` edge, so an unavailable/slow secret backend imposed a
full 45-second boot stall. The cloud credential hook then used `--refresh` to
restart both the daemon and shell as soon as the credential appeared; Dell's
daemon did not drain within its 90-second stop budget, adding another visible
stall. This was not a QEMU image-read delay. The currently running browser VM
also consumes roughly 8 GiB and ~49% of one sampled CPU view, but it is not on
the systemd boot critical chain.

The packaging fix removes the recipient `Before/After` boot gate while keeping
the service as a parallel best-effort lane. `--refresh` now materializes and
daemon-reloads the cloud credential without interrupting a live seat; only the
explicit operator `--restart` path invokes `try-restart`. Focused farm gates
`arch010-dell-boot-cloud-r1` and `arch010-dell-boot-role-r1` each passed 1/1,
and the helper syntax/self-test passed locally. No live reboot or new RPM
deployment is claimed by this slice.

## Workload authority cutover slice (2026-08-05)

The production VM/container lifecycle now has one active actuator and one
typed request path:

```text
legacy lifecycle_exec spawn: removed
legacy compute_registry inventory poller: removed
ActionWorker ServiceLifecycle: WorkloadOperationRequest → workload_compute
shell Front Door service lifecycle: WorkloadOperationRequest → workload_compute
worker census: 142 entries; tiered/dynamic: 77 (rank 0: 46, rank 1: 31)
```

The former replicated-file lifecycle executor and the virsh/podman inventory
poller are no longer spawned by `mackesd`; retaining those live paths would
permit a second VM/container authority to race `state/workloads/<node>`. The
ActionWorker and shell Front Door now normalize the target, derive the
canonical workload id/backend/action, bind a short-lived body-bound token, and
publish only `WORKLOAD_OPERATION_TOPIC`. The operation worker remains the sole
VM/container actuator and projection owner.

Farm verification on BigBoy passed after the census was updated for the
retired lanes:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-retire-compute-inventory-r4 \
  install-helpers/xcp-build.sh cargo test -p mackesd worker_role::tests:: -- --nocapture
result: 24 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-action-workload-r1 \
  install-helpers/xcp-build.sh cargo test -p mackesd workers::action::tests:: -- --nocapture
result: 35 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-frontdoor-workload-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui front_door_service_lifecycle -- --nocapture
result: 5 passed, 0 failed
```

The exact disposable farm fixtures were removed after each run; BigBoy `/home`
returned to approximately 77% used. `lint-workload-authority.sh`, the
worklist self-test/full lint, and `git diff --check` pass. This is source and
farm-gate evidence only: Dell has not been rebooted or redeployed with this
post-cutover source, so no live boot-time improvement or fleet acceptance is
claimed yet.

## Fedora 44 boot-fix deployment (2026-08-05)

The boot-fix source was cut on the BigBoy Fedora 44 container farm lane
(`arch010-dell-boot-fix-r1`). The base RPM was pulled locally and checked
before staging it to `DELL-LAPTOP` (`172.20.146.225`):

```text
artifact: magic-mesh-12.1.6-4.x86_64.rpm
size:     87,140,772 bytes (83.1 MiB)
sha256:   899ccbc5c0d07048b662f272e90260cb5d22b6439cf6cbd50dd2fb902f276743
payload:  verify-rpm-payload.sh payload — all checks passed
```

Dell matched that checksum, passed `rpm -Uvh --replacepkgs --test`, installed
the exact artifact, ran `systemctl daemon-reload`, and restarted `mackesd` and
the shell. The restart briefly reset the SSH transport, then both services
returned healthy:

```text
rpm:      magic-mesh-12.1.6-4.x86_64
mackesd:  active, NRestarts=0
shell:    active, NRestarts=0
```

The installed dependency graph now has `mackesd.service` ordered only after
`network-online.target` and `nebula.service`; the mesh-secret recipient is a
parallel best-effort Wants dependency and is absent from `mackesd`'s `After=`
chain. `mcnf-mesh-secret-recipient.service` likewise has no ordering edge that
can hold `mackesd` behind a 45-second recipient retry. The live Dell has not
been rebooted in this deployment, so the old boot's timestamps remain a
historical measurement; a reboot and a fresh `systemd-analyze critical-chain`
are still required to claim the measured boot-time reduction.

The exact farm fixture was removed after the artifact was pulled and verified.

## Authoritative Datacenter projection cutover (2026-08-05)

The Fleet Datacenter client no longer reads `event/vm/instances` or
`event/podman/containers`. It enumerates only `state/workloads/<node>`, validates
the versioned `WorkloadStateSnapshot`, and derives VM/container rows from the
typed backend and power dimensions. The shell therefore consumes the same
authoritative per-node projection as every other Workload client rather than a
second libvirt/Podman inventory lane.

`WorkloadOperationStatus` now carries its optional, validated approved
`image_ref`. This preserves a container's catalog identity for presentation
without exposing a host path or trusting an unvalidated status payload; absent
fields remain backward-compatible with prior persisted snapshots.

Focused BigBoy farm evidence:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-datacenter-workload-r2 \
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui datacenter::tests:: -- --nocapture
result: 27 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-workload-status-image-r1 \
  install-helpers/xcp-build.sh cargo test -p mackes-mesh-types workloads::tests:: -- --nocapture
result: 7 passed, 0 failed
```

Both exact farm fixtures were removed after their final run. This is source and
farm evidence only: the post-cutover Workloads package has not yet been cut or
installed on a live seat, and the remaining lifecycle/console producer
retirement and broader live acceptance gates remain Required.

## Typed correlated operation replies (2026-08-05)

The `action/workload/operation` worker now writes a typed reply to
`reply/<action-message-ulid>` after each targeted request. Accepted and identical
replay requests carry the authoritative durable `WorkloadOperationStatus`;
malformed, oversized, unauthorized, conflict, stale-generation, and journal
failures carry bounded `WorkloadOperationErrorCode` values. Request IDs are
recovered only when bounded and control-free, and raw request/provider text is
never copied into a refusal reply. A request for another node is left for its
own worker, preserving one authority per node.

Focused farm evidence:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-operation-replies \
  install-helpers/xcp-build.sh cargo test -p mackesd \
    --features async-services --lib workers::workload_compute::
result: 14 passed, 0 failed (3m40s)

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-workload-types \
  install-helpers/xcp-build.sh cargo test -p mackes-mesh-types workloads::tests::
result: 7 passed, 0 failed
```

The new hostile test proves accepted correlation, durable status, malformed
request refusal, and no adapter call on malformed input. The broader package
`cargo fmt --check` lane reported pre-existing formatting drift across the
already-dirty `mackesd` and mesh-types trees; the changed reply logic was
manually formatted and the source remains subject to the repository-wide
format gate. This slice is source/farm evidence only: Dell has not been
rebooted or redeployed with the new Workload reply code, and the remaining
caller migration, live Display1/KMS, pool/SELinux, and recovery transport proof
remain Required.

## Dell review staging (2026-08-05)

The review-only bundle at
`mm@172.20.146.225:~/magic-mesh-review/2026-08-05-drain-goal/` now includes
`workload/{workloads,workload_compute}.rs` alongside the Music slice, canonical
worklist, evidence, and authority gates. The installed Dell runtime was not
overwritten or rebooted; `magic-mesh-12.1.6-4.x86_64`, `mackesd.service`, and
`mde-shell-egui.service` remained present/active during the audit. Dell hashes
for the new Workload files match the local working tree:

```text
workload/workloads.rs       b0315f23f30f14e4fc8878bb1fcb885672ebc6db55c1481f15e4062605ead292
workload/workload_compute.rs d679863d1f33317d0a8d15158f04d6c33b3718b1480d1234186fb71c401cfd12
```

## Cancellation side-effect guard (2026-08-05)

The Workload reconciler now resolves a queued `Cancel` after deadline and
placement checks but before admission, Display1 broker creation, or any
libvirt/systemd adapter call. The concrete system adapter repeats the guard so
direct or replayed cancellation cannot define a VM, start a unit, or create a
node-local attachment server. This is deliberately limited to queued
cancellation; cancelling an already-running operation still needs a named
target-operation contract and adapter cleanup proof.

Focused farm verification used explicit isolated slots:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-cancel-r3-big \
  install-helpers/xcp-build.sh cargo test -p mackesd \
    --features async-services --lib workers::workload_compute::
result: 16 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=arch010-cancel-r2-types \
  install-helpers/xcp-build.sh cargo test -p mackes-mesh-types workloads::
result: 7 passed, 0 failed
```

The hostile cases cover queued cancellation without an actuator call, direct
system-adapter cancellation without a Display1 socket, Lighthouse rejection
for every action including `Cancel`, and the existing replay/authorization
paths. The parallel `cargo fmt --all -- --check` lane reported unrelated
pre-existing formatting drift across the dirty tree; no format claim is made.
The first BigBoy run also caught and corrected the ordering regression where an
early cancel branch bypassed Lighthouse rejection; the final 16-test gate is
the corrected result. An independent exact-test rerun on `172.20.0.90`
(`arch010-cancel-r4-exact`) was blocked before compilation by that farm
fixture's `No space left on device`; it is retained as an environment incident,
not a code result. No Dell/seat-15 runtime proof is claimed.

After the worklist paragraph was compacted to satisfy the canonical shape gate,
the governance checks were rerun on the farm workspace
`172.20.0.170:magic-mesh-farm-arch010-doc-gates-r1`:

```text
install-helpers/lint-worklist.sh --self-test: pass
install-helpers/lint-worklist.sh: items=17 remaining=17 blocked=0 needs_clarification=0
install-helpers/lint-doc-supersession.sh: clean
install-helpers/lint-workload-authority.sh: clean
```

## Explicit target-operation cancellation (2026-08-05)

The cancellation contract now carries a bounded `target_request_id`. A Cancel
request must name a distinct journaled operation, match its workload/node/
backend/resource tuple, and compare-and-swap its exact generation. The ledger
assigns the cancel request the next generation so the Workload projection keeps
one authoritative row rather than exposing a target and cancel duplicate.

Queued, Validating, and Admitting targets are cancelled journal-only before any
adapter boundary. A target past Defining is routed through the dedicated
`WorkloadActuator::cancel` seam; the system adapter destroys/undefines VM
targets or stops managed units, removes the Display1 attachment, and reports a
bounded retry while cleanup is incomplete. The cancel request completes only
after the target is terminal. These are source and fake-adapter contract tests;
live libvirt/Quadlet, Display1, restart/crash, and Dell/seat-15 cleanup proof
remain open.

Final focused farm verification used explicit isolated slots:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=workload-cancel-target-r4-big \
  install-helpers/xcp-build.sh cargo test -p mackesd \
    --features async-services --lib workers::workload_compute::
result: 17 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=workload-cancel-ledger-r3-small \
  install-helpers/xcp-build.sh cargo test -p mackesd \
    --features async-services --lib workload_reconciler::
result: 4 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=workload-cancel-contract-r1-types \
  install-helpers/xcp-build.sh cargo test -p mackes-mesh-types workloads::
result: 8 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=workload-cancel-shell-r1-big \
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui --tests workload_api::
result: 4 passed, 0 failed
```

The earlier 16-test cancellation result in this evidence is superseded by the
17-test corrected run above; its Lighthouse regression was fixed by supplying
the explicit target fixture required by the new contract.

## Typed Cuttlefish outer lifecycle and manifest-bound registration (2026-08-05)

The Android desired-state path now admits a production Cuttlefish adapter when
the daemon-owned host-local package manifest is present, bounded, symlink-free,
and exactly matches the desired image id and digest. The adapter is workload
scoped and uses the existing `CloudRunner`/libvirt lifecycle authority for
start, stop, reboot, and delete. Provision remains with the armed desired-state
and OpenTofu lane because the provider client is not allowed to invent a tfvars
document.

An active outer libvirt domain is projected as `Starting` with guest booting and
not-ready evidence. It is never projected as Android guest-ready from the
outer-domain state alone. The typed lifecycle contract permits stop/reboot and
destroy during the bounded starting/rebooting states; destroy resets the
provider generation to the contract's `Absent/generation=0` state. Guest
package-manager inventory, launch, inner display/session, and nested-KVM proof
remain unavailable and are still reported as open work in the canonical
worklist.

Focused farm verification used explicit isolated slots:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=android-cuttlefish-lifecycle-r1 \
  install-helpers/xcp-build.sh cargo test -p mackesd cuttlefish -- --nocapture
result: 12 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=android-cuttlefish-lifecycle-r1 \
  install-helpers/xcp-build.sh cargo test -p mackesd \
    verified_manifest_auto_registers_libvirt_cuttlefish_provider -- --nocapture
result: 1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=android-cuttlefish-types-r1 \
  install-helpers/xcp-build.sh cargo test -p mackes-mesh-types \
    android_provider:: -- --nocapture
result: 7 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=android-cuttlefish-format-r1 \
  install-helpers/xcp-build.sh sync
ssh mm@172.20.0.90 'cd ~/magic-mesh-farm-android-cuttlefish-format-r1 && \
  rustfmt --edition 2021 --check \
    crates/mesh/mackes-mesh-types/src/android_provider.rs \
    crates/mesh/mackesd/src/workers/cloud/verbs/cuttlefish.rs \
    crates/mesh/mackesd/src/workers/cloud/verbs/android.rs \
    crates/mesh/mackesd/src/workers/cloud/verbs.rs \
    crates/mesh/mackesd/src/workers/cloud/mod.rs'
result: pass for all five touched files
```

The `mackesd` focused build emits the repository's existing warning/dead-code
debt; no package-wide clippy-clean claim is made. The full workspace formatter
lane remains non-clean because of unrelated dirty-tree edits. No live
Cuttlefish guest, Dell, or seat-15 proof is claimed.

## Dell review staging refresh (2026-08-05)

The review-only bundle at
`mm@172.20.146.225:~/magic-mesh-review/2026-08-05-drain-goal/` is refreshed
with the current Workload contract, reconciler, worker, shell cancellation
constructor, canonical worklist, and this evidence. The installed Dell runtime remains untouched and was not
rebooted; package `magic-mesh-12.1.6-4.x86_64` and both
`mackesd.service`/`mde-shell-egui.service` remain active. Final bundle hashes
match the local review payload:

```text
workload/workloads.rs              b7644047a1ba03c0bcb5028167bc38375097e28996d29f1cf4642b2b2a6e1b9c
workload/workload_reconciler.rs    4480f2ff2a2f41d676699a3f2fcac589eccb71a34957f41b4cc46eddfbd4fbe9
workload/workload_compute.rs       4a307e55dedd5f21126e3b859440d27bfdc3db868032cba35a438686ad387c4a
workload/workload_api.rs           904e7389f226d847ff3c65974a7282e82dc9892bf8f24d3ba4924f8dbab3eedc
WORKLIST.md                        e11272171ad6c2d47cbd713dbf7f24d06b06efda2c0049f365e4c51e5293c706
```

The current review-only refresh also stages the Android/Cuttlefish slice under
`android/` and the Android packaging gates under `packaging/android/`. The
installed Dell runtime was not overwritten or rebooted. Current non-self
payload hashes match local files; the evidence file itself is verified against
the final staged hash during the refresh handoff:

```text
WORKLIST.md                        c7e8536c4c28dfd7837b841144fca814d1b157a08b19853d843439dd25bb0ef7
android/android_provider.rs        fc9ab8897c735f6d02d5cd61153a703be97e15b10abbfde56c1177f3ff7fc38d
android/cloud_mod.rs               82c620f0dda5fd64e562c58c0ac3741c7e0b2a6107f492ae122442ef9dfe967a
android/verbs.rs                   b6224e4e66386ea5c4226026b287df3237175dbe7cc425d6b2a1605c8393cfbd
android/android.rs                 3d2a762de6d08c96955a002a61b22067faffc2810c929b832eed63399561457b
android/cuttlefish.rs              8e1ad84a81ea1c2b574232ee5d7ef76b351b7d9c7a4e8ce45eee47c0073a7d3c
packaging/android/verify-manifest.sh
                                   12e640fcd8df033f5dd798e5bdb8b9cb30997dde3529c0255357bb634bb8fc55
packaging/android/verify-contract.sh
                                   2c82098cb7cb5a16cfa3a239ced443b4e535b48c8e7a0624b4a31e804396fdae
packaging/android/record-guest-tool-readiness.sh
                                   9954c4930a79d59254fc3ed81cc0967fe7e8a6cd53d1fa0cd716bfe98dd49fcf
```

## Bounded durable operation retention (2026-08-06)

The sole Workload operation journal now has a durable bound of 1,024 records.
Active operations and each workload's latest generation are never evicted.
When a new operation would exceed the bound, the reconciler removes only
superseded terminal history, ordered by generation and request ID. If every
record is active or is still the latest generation for its workload, admission
fails before the new record can be flushed or an adapter side effect can run.
The loader rejects an oversized persisted document as malformed. A retained
request remains an idempotent replay after pruning; replay outside the bounded
retention window is intentionally not promised.

Focused farm verification used explicit isolated slots:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-journal-retention-r1-big \
  install-helpers/xcp-build.sh cargo test -p mackesd \
    --features async-services --lib workload_reconciler
result: 7 passed, 0 failed; 4,393 filtered out

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-journal-retention-fmt-r1 \
  install-helpers/xcp-build.sh sync
ssh mm@172.20.0.90 'cd /home/mm/magic-mesh-farm-arch010-journal-retention-fmt-r1 && \
  rustfmt --check --edition 2021 \
    crates/mesh/mackesd/src/workload_reconciler.rs'
result: pass for the touched reconciler file
```

The hostile coverage includes retained-generation replay, a full active
journal that refuses mutation without changing disk state, and an oversized
persisted journal rejected before replay. This slice does not claim live
libvirt/Quadlet recovery, Display1/KMS presentation, caller migration, or
Dell/seat-15 acceptance; those remain `Remaining` in the canonical Worklist.
