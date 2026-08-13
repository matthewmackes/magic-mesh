# WL-FUNC-020 Android catalog staging crash recovery — r492

## Acceptance gap

The governed Android catalog importer persisted an admitted catalog through one
PID-derived `create_new` staging path. An unclean exit could leave that path in
place; later PID reuse then made every newer signed catalog fail before durable
last-good replacement. That converted harmless crash debris into a persistent
provider denial and prevented corrected-forward catalog authority.

## Implementation

`crates/mesh/mackesd/src/workers/android_catalog.rs` now searches a bounded set
of 32 private PID/suffix staging names. Existing entries are neither trusted,
followed, removed, nor overwritten. Non-collision I/O failures still fail
closed, and exhausting the bounded set returns an actionable error. The durable
write retains `create_new`, mode `0600`, `O_NOFOLLOW`, file sync, atomic rename,
and parent-directory sync.

The focused regression leaves a hostile stale first staging file in place,
persists and reloads a newly signed catalog through the next slot, and verifies
that the stale bytes were not modified.

## Farm gates

- `.50`, slot `func020-catalog-staging-recovery-r492`:
  `cargo test -p mackesd --lib workers::android_catalog::tests::stale_cache_staging_file_cannot_wedge_signed_catalog_updates -- --exact --nocapture`
  passed 1/1 (4,929 filtered out).
- `.170`, slot `func020-catalog-clean-clippy-r492`:
  `cargo clippy -p mackesd --lib -- -D warnings` passed from a clean detached
  branch worktree carrying only this source change. The first `.90` attempt was
  rejected because an unrelated concurrent `ipc/files.rs` edit contained
  unused imports; that file was absent from the authoritative clean rerun.
- `.196`, slot `func020-catalog-staging-fmt-r492`:
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/android_catalog.rs`
  passed.

## Remaining epic acceptance

This closes one catalog crash-recovery/provider availability boundary. FUNC-020
still requires release and guest packaging, nested-KVM execution, Remote
Sessions attachment, and the deferred post-release Cuttlefish/VDI/isolation and
recovery proof described by the canonical worklist.
