# WL-FUNC-017 offline tile catalog binding — 2026-08-09

## Correction

`OfflineTileCache` previously authorized a tile only when storing it. Lookup
could therefore return bytes admitted by an expired or replaced catalog until
the independent cache TTL elapsed. Cache index schema 2 now persists the exact
admitting catalog SHA-256. Every lookup requires that digest and the current
catalog's region, zoom, and expiry approval; mismatch removes the entry and
returns `CatalogRejectedRemoved` instead of stale map bytes. Legacy schema-1
indexes are atomically replaced by an empty schema-2 index: their unbound
payloads are quarantined/removed when their legacy identities remain safely
parseable, and no legacy entry is admitted. Invalid legacy metadata or a lower
new quota cannot make application startup fail; the active cache opens empty
and can accept newly catalog-bound tiles.

## Farm proof

- Host: `172.20.0.90`
- Slot: `func017-offline-catalog-bind-r1-20260809`
- Catalog replacement/expiry regression: passed. It restarts the cache,
  replaces the verified catalog, proves removal, then proves catalog expiry.
- Schema-1 migration regression: passed. A legacy payload larger than the new
  quota is removed, startup succeeds with an empty schema-2 index, legacy
  lookup misses, and a fresh catalog-bound tile stores and returns normally. A
  malformed legacy entry also opens empty; its unparseable payload is left
  physically isolated and cannot be addressed through the rewritten index.
- Complete `offline_cache::tests`: 7 passed, 0 failed.
- Exact-file Rustfmt check: passed.
- Scoped local `git diff --check`: passed.
- Source SHA-256:
  `39801883c03c9ac38a8e02485d837663efcaf94a2faa40d855d6f85b1469167b`.

No live provider or packaged offline-region claim is made by this bounded
correction.
