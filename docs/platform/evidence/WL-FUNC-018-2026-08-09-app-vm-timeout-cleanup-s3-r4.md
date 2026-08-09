# WL-FUNC-018 S3 — App VM timeout cleanup (r4)

Date: 2026-08-09

## Correction

The Workload reconciler previously made an operation terminal when its readiness deadline expired, even after crossing the adapter side-effect boundary. That could leave an App VM or Display1 lease alive behind a `Failed` projection.

Expired post-admission operations now revoke their exact attachment immediately and enter the authoritative `Stopping` phase. The same generation remains nonterminal and blocks duplicate opens until idempotent adapter cancellation proves the backend is stopped with no attachment; only then does the operation become `Cancelled`. No parallel lifecycle state or desired-state write was added.

## Focused farm proof

Machine 193 (`172.20.0.90`), slot `func018-cleanup-r4`:

```text
cargo test -p mackesd --lib --features async-services \
  expired_app_vm_open_revokes_lease_and_blocks_duplicates_until_cleanup -- --nocapture

test workers::workload_compute::tests::expired_app_vm_open_revokes_lease_and_blocks_duplicates_until_cleanup ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 4364 filtered out
```

The hostile fixture proves a slow first cleanup attempt publishes `Stopping`/`Stopping`, revokes the VDI lease, preserves generation 1, refuses a distinct duplicate open, converges on the second cleanup attempt, and treats a same-request replay as read-only.

Final source SHA-256:

- `workload_compute.rs`: `0c67a50233f3a36f01140669ba5529566f36afdda2edd650f1d7f47be53bc995`
- `workloads.rs`: `c543674e890d20bea1c0a7d69056697785fbdd6167ab0bf4de3c6eec74eb49b0`

`git diff --check` passed for both files. Existing crate warnings were unchanged and non-fatal.

## Remaining

S3 still needs a real App VM lifecycle trace through start, readiness, VDI attach, close/policy stop, and crash recovery. S4 UX, S5 package/security, and five-seat live proof remain open.
