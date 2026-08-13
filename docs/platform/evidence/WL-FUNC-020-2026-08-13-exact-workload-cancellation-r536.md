# WL-FUNC-020 — exact Workload cancellation handoff (r536)

Date: 2026-08-13

## Production change

The universal-resource router previously authenticated an Android cancellation's
`cancels_request_id` but discarded that identity before publishing to the
Android lifecycle authority. The authority therefore refused every Cancel and
could not reclaim an in-flight outer-VM operation.

The router now copies the already-authenticated cancellation target into the
closed Android lifecycle request before signing it. The Android authority:

- requires a non-zero projected workload generation;
- requires one explicit prior request ID and refuses implicit or self targets;
- publishes `WorkloadOperationAction::Cancel` through the sole typed Workloads
  operation topic; and
- preserves the exact workload, node, libvirt backend, resource profile,
  generation, and target request ID for the Workloads reconciler's durable
  cross-check before actuator cancellation.

No direct adb, QEMU, libvirt, or guest-agent cancellation path was added.

## Farm gates

- `172.20.0.170`, slot 2: `cargo check -p mackesd --all-targets` — passed.
- `172.20.0.130`, slot 1: `cargo clippy -p mackesd --all-targets -- -D warnings`
  — passed before the BigBoy low-space advisory; no further BigBoy work was
  started.
- `172.20.0.50`, slot 1: exact-file Rust 1.94 `rustfmt --check` for both owned
  modules — passed. Package-wide formatting was not claimed because unrelated
  files already differ from rustfmt.
- `172.20.0.50`, slot 1: focused resource-router cancellation test — passed
  1/1 (`4975` filtered out).
- `172.20.0.50`, slot 2: focused Android lifecycle cancellation test — passed
  1/1 (`4971` filtered out).
- Local `git diff --check` — passed.

The first BigBoy focused-test attempt was terminated by the urgent low-space
recovery before test execution and is not counted as evidence. Its two shared
slot workspaces also contained active commands from other owners, so they were
not deleted without exclusive ownership proof.

## Residual WL-FUNC-020 criteria

- Audit and close any remaining concrete retry, guest-input, or crash cleanup
  gaps that can be implemented before release.
- Consume the real signed Cuttlefish image and deterministic guest DEBs in the
  first full release.
- After release, perform the deferred non-blocking nested-KVM boot, app
  lifecycle, VDI input/audio/reconnect, isolation, upgrade, and one-node live
  acceptance matrix.
