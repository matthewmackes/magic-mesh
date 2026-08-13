# WL-FUNC-020 Android catalog/provider slice audit — 2026-08-13

## Scope

Audited the bounded `mackesd` Android catalog/provider surface:

- `crates/mesh/mackesd/src/workers/android_catalog.rs`
- `crates/mesh/mackesd/src/workers/cloud/android_provider.rs`
- `crates/mesh/mackesd/src/workers/cloud/verbs/android_lifecycle.rs`

No source change was made. The request was limited to one Android
catalog/provider/lifecycle module and one evidence file; unrelated dirty files
and the canonical worklist were left untouched.

## Current executable coverage

The owned modules already provide and test:

- signed catalog admission with pinned Ed25519 identity, expiry, revision and
  catalog-identity continuity;
- no-follow, bounded durable catalog state and corrupt-cache fail-closed
  restart behavior;
- late/replaced Bus replay with generation checks and retry-safe publication;
- image/package provenance matching, regular-file digest verification, KVM and
  nested-virtualization checks, resource-capacity checks, and provider-health
  refusal diagnostics;
- typed Android Workload Start/Stop delegation with generation-bound admission.

These behaviors are already recorded by the FUNC-020 evidence entries in
`docs/platform/WORKLIST.md`, including the BigBoy provider and lifecycle gates.
Adding another unit or hostile test in the same module would duplicate those
assertions rather than advance an acceptance criterion.

## Precise blocker

The remaining FUNC-020 acceptance is outside this bounded code scope:

1. signed release-image/package artifacts and their installer admission;
2. guest packaging and nested-KVM execution on an Android/Cuttlefish host;
3. Remote Sessions/VDI attachment and input/audio behavior;
4. live SELinux/cgroup/device-isolation, reconnect, upgrade, and cleanup proof.

Those require release/package/guest fixtures and live hardware or an approved
recorded fixture. The catalog/provider module cannot honestly manufacture those
artifacts or claim readiness from a local unit test. No safe substantive code
gap was found, so no farm cargo gate was run and no busywork was created.

## Farm state at audit

`install-helpers/farm-topology.sh table` and `install-helpers/farm.sh status`
reported 5/5 build VMs up and 0/10 heavy slots active (10 free): `.50` 2,
`.90` 2, BigBoy `.130` 3, `.170` 2, `.196` 1. No unique source gate existed
for this blocker.

## Remaining acceptance

FUNC-020 remains `Remaining`. Release artifacts, guest/nested-KVM packaging,
remote-session attachment, live VDI/DRM/input/audio behavior, isolation,
reconnect/upgrade/cleanup, and the documented post-release proof remain open.
