# WL-ARCH-010 contract, restart, and Display1 hardening — 2026-08-09

## Outcome

`WorkloadOperationStatus` now rejects an attachment lease whose workload ID
does not match the status workload. Generation matching alone was insufficient:
a foreign runtime lease at the same generation could otherwise enter a valid
typed projection and be offered to the wrong consumer.

The reconciler now resumes admission from a durably journaled `Validating`
phase. A process crash after the Queued-to-Validating flush previously left the
operation in the ledger but outside the restart scan, so it could remain stuck
without either a backend effect or an actionable failure. The recovery test
reopens the ledger at that exact crash boundary and proves one actuator call.

Display1 broker startup no longer unlinks an existing lease socket before it
knows whether the endpoint is stale. A duplicate server now refuses while the
original listener remains reachable; a connection-refused socket left by a
dead listener is removed and rebound. This preserves single ownership without
sacrificing crash recovery.

The unreferenced `install-helpers/provision-mesh-vm.sh` was deleted. It copied
arbitrary images into libvirt storage and invoked `virt-install` outside typed
admission, journaling, and the sole Workload actuator. The authority inventory
and lint now reject restoring that bypass.

The Browser image deployment self-test now executes its embedded deployment
receipt writer and validates the resulting schema-v2 artifact. This covers the
operator-visible receipt path rather than checking only shell syntax and helper
predicates.

## Farm verification

- Machine 9 (`172.20.0.50`), slot `arch010-s2-attachment-bind-r1`:
  `cargo test -p mackes-mesh-types --locked workloads::tests -- --nocapture`
  passed 12/12.
- BigBoy (`172.20.0.130`), slot `arch010-s3-validating-restart-r2`: final
  `cargo test -p mackesd --lib --features async-services workload_compute
  --locked -- --nocapture` passed 36/36.
- BigBoy (`172.20.0.130`), slot `arch010-s6-duplicate-socket-r2`: final
  `cargo test -p mackesd --lib --features async-services
  display1_broker::tests --locked -- --nocapture` passed 9/9 after the exact
  synchronized rerun.
- Machine 193 (`172.20.0.90`), slot `arch010-s7-browser-receipt-r1`:
  `deploy-image.sh --self-test`, the complete Browser VM contract verifier,
  shell syntax, workload-authority lint, and its hostile self-test passed.
- Machine 193 (`172.20.0.90`), slot `arch010-r12-lints`: exact final
  contract/reconciler `rustfmt --check`, Browser deploy/lint shell syntax,
  workload-authority lint and self-test, worklist lint and self-test, the
  Browser VM contract verifier, and documentation-supersession lint passed.
  The live worklist result remained `items=18 remaining=18 blocked=0
  needs_clarification=0`.

## Source hashes

- `ed7e262854782b5b88350dc0d8e28969252927b9d500786e30beb44b5e887cf8`
  — `crates/mesh/mackes-mesh-types/src/workloads.rs`
- `9e67c47605d2d6d3b6229c75c03862fb6028553ed18adf976ca72d312904945d`
  — `crates/mesh/mackesd/src/workers/workload_compute.rs`
- `7b3b851dacb4e4e05ca61bf6c2900c68f6c54ea2721eb68b2d8b980c00079162`
  — `crates/mesh/mackesd/src/display1_broker.rs`
- `5dffefe2da638c0f13204ec74d486463af546bee68e8e7a622a873b29e7489b1`
  — `packaging/browser-vm/deploy-image.sh`
- `a60dd4397d4541a1077d728b2d4d6cdeea8c932fe246dbfbef9c0b892e594ba5`
  — `install-helpers/lint-workload-authority.sh`
- `8bee58cafc6b97c9318b55e4cff7c29f471c79633876640fc2f9785670b5efbd`
  — `docs/platform/workload-authority-inventory.md`

## Remaining boundary

This closes concrete S2, S3, S6, and S7 failure windows but does not close
WL-ARCH-010. Full crash injection, native frame/input/audio/clipboard proof,
package install/upgrade, and the required live-seat lifecycle matrix remain.
