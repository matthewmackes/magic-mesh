# WL-ARCH-010 Quadlet runtime rematerialization — 2026-08-13

## Gap closed

Rootful Quadlet source units are intentionally materialized under
`/run/containers/systemd`. After a host reboot or runtime-state loss, the
durable Workload journal could therefore retain an admitted container Start in
`WaitingForGuest`, or a Restart in its journaled `Starting` phase, while the
generated unit no longer existed. Recovery previously polled the absent service
or issued `systemctl start` against the missing unit until the bounded operation
failed.

`workload_compute` now rematerializes the exact catalog-approved OCI-backed
Quadlet and reloads systemd before retrying start in those two states. The
decision is fail-closed: it cannot recreate a VM, an already-running service, a
cleanup operation, or a Restart that has not durably crossed into `Starting`.
The normal bounded operation deadline and adapter retry ceiling remain the sole
retry budget.

## Farm evidence

- `.90`, slot `arch010-quadlet-runtime-recovery-clippy-r497`: final-source
  focused regression
  `workers::workload_compute::tests::quadlet_runtime_loss_recovers_only_admitted_start_phases`
  passed 1/1; 4,946 tests were filtered out. An earlier invocation that selected
  zero tests was rejected and is not evidence.
- `.90`, the same final-source workspace: strict
  `cargo clippy -p mackesd --lib --features async-services -- -D warnings`
  passed.
- `.170`, slot `arch010-quadlet-runtime-filelines-r497`: rustfmt rendered the
  complete authorized source to a temporary file and a zero-context diff proved
  every newly added recovery hunk clean. Package-wide `cargo fmt` remains red on
  inherited unrelated drift and was not claimed.
- `.196`, slot `arch010-quadlet-runtime-module-r497`: a broader module run was
  started when the host was restored, but later SSH/sync saturation prevented a
  trustworthy completion. It is explicitly not claimed as evidence.
- `git diff --check` passed before the explicit-path commit.

## Scope and residual acceptance

The implementation changes only
`crates/mesh/mackesd/src/workers/workload_compute.rs`. Concurrent runtime-probe,
service-catalog, weather, and unrelated evidence work remained unstaged.

Remaining WL-ARCH-010 acceptance is the package/repository gate, real
libvirt/Quadlet StartAndAttach readiness, native KMS/Display1 recovery, and the
deferred post-release installed-seat/fleet lifecycle matrix.
