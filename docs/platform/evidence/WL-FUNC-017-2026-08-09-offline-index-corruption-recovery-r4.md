# WL-FUNC-017 S5 — offline index corruption recovery (r4)

## Missing boundary corrected

`OfflineTileCache::open` previously returned an error for malformed
current-generation `index.json` metadata. Because this cache is disposable and
the index is its sole admission authority, one truncated or structurally
hostile regular index could prevent Maps from reopening the cache indefinitely.

Current schema-2 metadata corruption now follows a bounded recovery path. The
regular index is atomically renamed out of authority, an empty schema-2 index is
atomically installed, and the displaced metadata is removed. The cache then
admits tiles only through the existing verified-catalog and digest checks. I/O
errors, non-regular index paths, and valid indexes that exceed a newly reduced
quota still fail visibly instead of being mislabeled as corruption.

The catalog-generation boundary remains fail closed: an unsupported future
schema is not rewritten or removed by an older reader. No network I/O was added
and `OfflineTileCache` remains the only cache authority.

## Focused farm proof

- Host: machine 193 (`172.20.0.90`).
- Slot: `func017-index-recovery-r4`.
- Exact command:
  `cargo test -p mde-maps-location-egui offline_cache::tests::corrupt_current_index_recovers_empty_without_admitting_hostile_metadata -- --exact`
- Library result: **1 passed, 0 failed, 300 filtered out**.
- Binary result: **0 tests, 0 failed**.
- The regression exercised truncated JSON and malformed schema-2 structure,
  proved recovery to an empty current index, and proved verified store/lookup
  still works afterward. It also proved schema 65535 returns the unsupported
  error while preserving the original bytes exactly.
- Owned-file `git diff --check`: passed.
- Source SHA-256:
  `39f93ed3fb267f6e50441250e6358bb0efe2220245efa3a307a5ac4263eca888`.

The focused build emitted the crate's existing non-fatal missing-documentation
and dead-code warnings. No live offline-region or provider proof is claimed;
WL-FUNC-017 remains `Remaining` for those broader acceptance items.
