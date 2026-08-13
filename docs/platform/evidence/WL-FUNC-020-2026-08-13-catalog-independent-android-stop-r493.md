# WL-FUNC-020 catalog-independent Android Stop — r493

- Date: 2026-08-13
- Scope: `crates/mesh/mackesd/src/workers/cloud/verbs/android_lifecycle.rs`
- Prior slice reviewed: `02f004b5` (Android catalog staging crash recovery)

## Implemented boundary

Android outer-VM cleanup no longer depends on loading the current signed release
catalog. `Start` still requires the admitted catalog and exact desired
image/package provenance. `Stop` instead requires the existing strict Android VM
desired declaration, an authenticated Cloud capability, a non-zero expected
Workload generation, the exact local placement, and the sole typed Workloads
operation lane.

This preserves fail-closed launch admission while allowing a generation-bound
Stop to reclaim an already admitted outer VM when catalog state is expired,
corrupt, absent, or otherwise unavailable. No direct libvirt/provider action was
added.

## Farm gates

- `.130`, slot `func020-catalogless-stop-test-r493b`: focused regression
  `stop_remains_available_when_release_catalog_is_unavailable` — passed 1/1
  (4,933 filtered out).
- `.170`, slot `func020-catalogless-stop-clippy-r493`: strict
  `cargo clippy -p mackesd --lib -- -D warnings` — passed after adding the
  test-only boundary to the catalog-injection helper.
- `.170`, slot `func020-catalogless-stop-filefmt-r493`: file-scoped
  `rustfmt --edition 2021 --check` — passed after applying canonical formatting
  within the assigned file.

The initial package-wide fmt check exposed unrelated committed formatting drift
outside this slice. The initial clippy run identified the new test helper as
production dead code; the helper was correctly restricted with `#[cfg(test)]`
and strict clippy then passed without a lint suppression. The first corrected
focused-test compile exposed an unrelated committed moved-value error in
`workers/clock.rs`; the compiler-suggested `peer.clone()` prerequisite was
applied only in the disposable `.130` workspace so the Android regression could
execute. No Clock change is included in this slice.

## Remaining epic acceptance

Typed Cancel/Retry correlation, guest package/release artifacts, remote-session
attachment, nested-KVM execution, isolation/package proof, and post-release live
Cuttlefish/VDI proof remain. This slice closes only the provider/catalog-loss
Stop cleanup gap.
