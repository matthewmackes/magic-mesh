# WL-FUNC-020 Android workload-boundary admission — 2026-08-13

## Result

Android provider preflight now admits only an exact `AndroidVm` Workload with
host-network isolation enabled. A service container, other delivery class, or
an unisolated declaration can no longer replay the signed Android image fields
and receive a false `Ready` placement row.

This closes a production S2 boundary in
`crates/mesh/mackesd/src/workers/cloud/android_provider.rs`: image identity,
digest, package provenance, capacity, KVM, and provider health were already
checked, but the Workload delivery class and isolation contract were not.
Refused declarations publish `Unavailable` with
`DesiredImageMismatch`; they never advance to artifact, KVM, capacity, or
provider-health admission.

## Farm gates

- BigBoy `.130`, slot `func020-boundary`:
  `cargo test -p mackesd provider_refuses_non_android_or_non_isolated_workload_replay -- --nocapture`
  passed 1/1 with 4,952 filtered library tests. The regression covers both a
  `ServiceContainer` identity replay and an `AndroidVm` with isolation disabled.
- `.50`, slot `func020-clippy`:
  `cargo clippy -p mackesd --lib -- -D warnings` passed.
- `.90`, slot `func020-filefmt`:
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/cloud/android_provider.rs`
  passed after a farm sync.
- `git diff --check` passed.

The broader `cargo fmt --all -- --check` exposed pre-existing concurrent
format drift across unrelated crates, so it was not used to mischaracterize
this file-scoped slice and no out-of-scope file was changed.

## Remaining acceptance

FUNC-020 still requires first-release integration, then the explicitly
deferred non-blocking installed one-node proof for nested KVM/Cuttlefish,
signed package identity, guest readiness, Workloads and Remote Sessions VDI,
input/audio, provider loss, restart/reconnect/upgrade, isolation, and cleanup.
No live or release proof is claimed by this source gate.
