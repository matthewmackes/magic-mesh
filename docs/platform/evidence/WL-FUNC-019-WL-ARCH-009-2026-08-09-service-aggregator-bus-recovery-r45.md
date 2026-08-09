# Service Aggregator Bus recovery — r45

Date: 2026-08-09

Baseline: `5b9d97d89b49b6447184eda2f0cada67ae7bd5e9`

## Corrected semantics

- Construction resolves the configured/user Bus root to one concrete path and falls back to `mde_bus::SYSTEM_BUS_ROOT` when no user data root is available. `None` can no longer become a permanent absent-source/no-op-publish mode.
- Startup keeps the same worker alive while an unresolved or unopenable Bus recovers. Retry waits are shutdown-aware and exponentially bounded from 10 ms through 2 s.
- The worker retains one `Persist` handle and follows a recreated Bus index. Pre/post inode and filesystem checks reject missing, unstatable, non-file, or unreopened replacement indexes rather than reading or writing an orphaned SQLite view.
- Every cycle stages the local service fold, strict latest desktop/SSH-X11/UPnP reads, universal catalog derivation, production source-adapter augmentation, client-capability admission, content digest, discovery projection, optional publisher attestation, validation, and JSON encoding before any mirror publication.
- Missing retained topics remain valid empty source states. A read error, missing retained body, strict decode error, catalog/adapter/capability/discovery failure, publisher-key backend error, or attestation mint/validation error produces zero publications and leaves both successful input state and success time unchanged. A deliberately undistributed publisher key remains the existing compatibility case: catalog/discovery publish without an authenticated proof.
- Publications use explicit `Persist::write` results. Any write failure leaves `last` and `last_pub_at` unchanged, making the complete staged cycle immediately retryable on the next poll.
- Successful state records include the three retained source ULIDs. A corrected external retained write therefore counts as changed input and publishes on the next cycle rather than waiting for the heartbeat. Unchanged successful inputs still retain change/heartbeat suppression.

## Changed file

- `crates/mesh/mackesd/src/workers/service_aggregator/mod.rs`
  - SHA-256: `b5880a66e8cfd6c4c9897feb0f8727aac14de9d6cda18ad55eff73102f96ff75`

## Verification

Farm topology reported BigBoy with three free heavy slots. The user-supplied BigBoy address `192.168.23.130` was attempted first with explicit slot `service-aggregator-bus-r45`, but SSH port 22 timed out. The farm inventory's address for the same BigBoy build VM, `172.20.0.130`, was then used with the required explicit slot.

Initial routed commands used this form:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=service-aggregator-bus-r45 ./install-helpers/xcp-build.sh cargo test -q -p mackesd --features async-services --lib <exact-test> -- --exact --nocapture
```

Farm rustfmt:

```text
ssh mm@172.20.0.130 'cd /home/mm/magic-mesh-farm-service-aggregator-bus-r45 && cargo fmt -- crates/mesh/mackesd/src/workers/service_aggregator/mod.rs'
PASS
```

Post-format exact tests in the same BigBoy slot:

```text
workers::service_aggregator::tests::final_retained_source_failure_publishes_nothing_and_advances_nothing
PASS: 1 passed; 0 failed

workers::service_aggregator::tests::write_failure_keeps_cycle_immediately_retryable
PASS: 1 passed; 0 failed

workers::service_aggregator::tests::same_worker_recovers_late_external_and_replaced_bus
PASS: 1 passed; 0 failed

workers::service_aggregator::tests::tick_loop_exits_promptly_on_shutdown
PASS: 1 passed; 0 failed

workers::service_aggregator::tests::publish_cycle_writes_validated_catalog_and_discovery_projection
PASS: 1 passed; 0 failed

workers::service_aggregator::tests::malformed_or_invalid_retained_desktop_state_suppresses_resource_mirrors
PASS: 1 passed; 0 failed

workers::service_aggregator::tests::publication_mints_and_retains_a_publisher_attestation_from_secret_store
PASS: 1 passed; 0 failed
```

Scoped checks:

```text
git diff --no-index --check /dev/null crates/mesh/mackesd/src/workers/service_aggregator/mod.rs
git diff --check -- crates/mesh/mackesd/src/workers/service_aggregator/mod.rs docs/platform/evidence/WL-FUNC-019-WL-ARCH-009-2026-08-09-service-aggregator-bus-recovery-r45.md
PASS: no output
```

Only pre-existing/concurrent crate-wide warnings were emitted. No commit or push was performed, and `docs/platform/WORKLIST.md` was not edited.
