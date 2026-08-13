# WL-ARCH-010 runtime mount identity — r498

Date: 2026-08-13

Branch: `agent/drain-worklist-20260725`

Starting revision: `3536c690ca84acdcc28751c81f66c2473a7c7982`

## Executable gap

The live installed/running application publishers use
`runtime_probe::is_meshfs_mounted` before writing their workload-adjacent
inventory to the replicated root. The guard accepted any matching
`/proc/mounts` row. Stacked mounts with the same target therefore remained
ambiguous: the checked filesystem could be shadowed by another mount before
publication, allowing inventory to land outside the intended replicated
runtime identity.

## Implementation

`crates/mesh/mackesd/src/workers/runtime_probe.rs` now:

- reads the calling process's `/proc/self/mountinfo` namespace;
- decodes mountinfo path escapes before exact target comparison;
- requires exactly one matching mount target;
- requires the target itself to be an existing non-symlink directory; and
- fails closed for missing, malformed, or stacked mount identities.

The focused hostile regression covers one escaped valid target, two stacked
identities, malformed mountinfo escaping, and a nonmatching target. This is a
reachable boundary used by `AppsInstalledWorker::tick_once` and
`AppsRunningWorker::tick_once`; no unused parser or duplicate test seam was
added.

## Farm gates

- `.170`, slot `arch010-runtime-mount-identity-clippy-r498` (reused after its
  Clippy gate):
  `cargo test -p mackesd --lib workers::runtime_probe::tests::mount_readiness_rejects_ambiguous_or_malformed_identity -- --exact`
  — passed 1/1; 4,947 filtered out. The initial `.90` cold-link attempt was
  interrupted and superseded rather than allowed to retain a farm lane.
- `.170`, slot `arch010-runtime-mount-identity-clippy-r498`:
  `cargo clippy -p mackesd --lib -- -D warnings` — passed.
- `.90`, slot `arch010-runtime-mount-identity-fmt-r498`:
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/runtime_probe.rs`
  in the synced isolated workspace — passed.
- `.50` safely refused the formatting sync at 5.0 GiB available, below the
  helper's 8-GiB floor; the unique gate was rerouted to `.90`.

## Remaining acceptance

This closes only ambiguous local runtime mount admission. `WL-ARCH-010` still
requires package/repository gates, real libvirt/Quadlet `StartAndAttach`
readiness, native KMS/Display1 recovery, and the deferred non-blocking
post-release installed-seat lifecycle matrix.
