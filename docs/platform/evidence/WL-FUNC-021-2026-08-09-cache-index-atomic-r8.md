# WL-FUNC-021 cache-index atomic persistence — r8

Date: 2026-08-09

Revision inspected: `c36e4002396a9e5ac10eb2238592f78e0f44b415` plus this uncommitted lane.

## Production change

`mde-musicd` now persists `music-cache/index.json` through a uniquely named
sibling file, syncs the complete JSON bytes, atomically renames the sibling,
and syncs the parent directory. A failed replacement removes its temporary
file and leaves the last-good index authoritative. This protects offline-track
identity, pin state, LRU timestamps, and eviction accounting from a daemon or
host failure during an index update.

The implementation is runtime-reachable through every existing cache index
writer: completed downloads, cached playback/LRU updates, pin/unpin actions,
track removal, and cache garbage collection.

## Focused farm verification

Machine 9 (`172.20.0.50`), slot `func021-cache-index-r8-test`:

```text
cargo test -p mde-musicd cache::tests -- --nocapture
16 passed; 0 failed; 219 filtered out
```

The failure-injection case
`cache::tests::failed_index_replace_preserves_last_good_cache_authority`
executed within that suite and proved both last-good preservation and temporary
file cleanup. An exact-file Rust formatting check also passed on machine 9:

```text
rustfmt --edition 2021 --check crates/services/mde-musicd/src/cache.rs
```

No provider, physical renderer, live seat, package, commit, push, or active
worklist-file claim is part of this checkpoint.
