# WL-ARCH-010 Workload cleanup idempotence — 2026-08-06

This slice hardens the node-local libvirt actuator's cleanup boundary. It does
not claim live libvirt, Display1/KMS, Dell, or seat acceptance.

## Implementation

`crates/mesh/mackesd/src/workers/workload_compute.rs` now treats the normal
libvirt result for destroying an already-stopped domain as an idempotent
cleanup condition. `virsh destroy` can report `domain is not running` even
though the subsequent `undefine` and managed-overlay cleanup are safe and
required. The helper also centralizes the accepted absent/stopped diagnostics
and continues to reject unrelated failures such as a virtqemud permission
error.

This keeps cancellation and corrected-forward cleanup from stranding a stopped
domain or leaving its managed overlay behind while preserving fail-closed
behavior for authoritative backend errors.

## Farm verification

The changed Workload lane ran on `.90` in isolated slot
`workload-cleanup-idempotence-20260806-r1`:

```text
MCNF_BUILD_HOST=172.20.0.90 \
MCNF_BUILD_SLOT=workload-cleanup-idempotence-20260806-r1 \
./install-helpers/xcp-build.sh cargo test -p mackesd workload_compute -- --nocapture

23 passed, 0 failed, 0 ignored, 4,383 filtered out
```

The new regression is
`workers::workload_compute::tests::libvirt_cleanup_treats_absent_and_stopped_domains_as_idempotent`.
The farm emitted existing warnings only. `git diff --check` passed for the
changed files.

## Remaining proof

The test is a command-policy/contract proof, not a live `virsh` invocation.
Live libvirt/virtqemud cleanup, crash/restart recovery, native Display1/KMS
attachment, and Dell/seat-15 acceptance remain open under WL-ARCH-010.

## Source hash at capture

```text
77e27185900b79fe57ea7710840bf51a0a4d0c7059a0e7aa82dbdc5abbd77055  crates/mesh/mackesd/src/workers/workload_compute.rs
```
