# WL-FUNC-017 — restart-safe physical offline-cache quota (r515)

Date: 2026-08-13

Branch: `agent/drain-worklist-20260725`

Source scope: `crates/desktop/mde-maps-location-egui/src/offline_cache.rs`

## Gap closed

The offline cache previously reconstructed quota usage from index-declared byte
lengths only. A crash after index publication but before retired-payload cleanup,
or corrupt-index recovery to an empty index, could leave regular tile files in
the cache tree that consumed disk while being invisible to quota admission.
An otherwise valid index could also assign a future `last_access_ms` and protect
that entry from deterministic LRU eviction during the next transaction.

`OfflineTileCache::open` now performs one bounded, non-symlink-following cache
tree reconciliation before returning authority. Indexed payloads survive only
when their real file type and byte length match admission. The reduced index is
published before invalid payloads are removed, and every other regular file in
the dedicated cache tree is quarantined and removed as an orphan. Traversal is
depth- and entry-bounded, so hostile directory growth fails closed instead of
creating unbounded startup work. A same-size byte replacement remains charged
to quota and is still digest-verified before lookup. The next store transaction
also excludes future-dated access metadata before calculating quota and
deterministic eviction.

The hostile regression creates two quota-filling admitted tiles, an orphan that
models a same-size retired payload, and future LRU metadata. Restart removes the
orphan; the next admission revokes the manipulated entry without exceeding the
quota; a subsequent syntactically corrupt index recovers empty and removes all
now-unindexed payloads.

## Farm evidence

- BigBoy `.130`, slot `func017-offline-quota-r515-test`:
  `cargo test -p mde-maps-location-egui offline_cache::tests::restart_reconciles_orphans_and_future_lru_before_quota_admission -- --exact --nocapture`
  passed **1/1** (`320` filtered).
- `.170`, slot `func017-offline-quota-r515-clippy`:
  `cargo clippy -p mde-maps-location-egui --all-targets -- -D warnings`
  passed.
- `.170`, slot `func017-offline-quota-r515-fmt`:
  `cargo fmt -p mde-maps-location-egui -- --check` passed after the first run
  identified two touched-file formatting differences and the exact rerun
  verified their correction.
- Local `git diff --check` passed. The local dev host has no Cargo binary, so no
  local Rust build/test was attempted.

The initial `.90` Clippy dispatch was stopped when farm reconciliation exposed
three Cargo jobs against that cap-2 node. No result from that interrupted run is
claimed. Its process was terminated, ownership was checked, and only the
disposable `func017-offline-quota-r515-clippy` workspace was removed before the
unique gate was rerouted to `.170`.

## Remaining epic acceptance

This closes S5's physical quota/restart reconciliation gap. WL-FUNC-017 still
requires first-release Maps/weather integration and package verification.
Installed one-seat offline map/route, provider-loss, restart, sleep/rejoin,
MG90, weather, and visual proof remains deferred and non-blocking until after
the first full release under the operator's current acceptance policy.
