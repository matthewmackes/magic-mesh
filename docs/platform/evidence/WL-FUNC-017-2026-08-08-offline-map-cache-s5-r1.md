# WL-FUNC-017 S5 verified offline map cache — 2026-08-08

Maps now has a bounded offline catalog/cache authority with deterministic
region and XYZ tile identities, approved-provider admission, SHA-256 content
verification, strict path/size/count bounds, atomic index replacement,
quota/LRU/age eviction, and verified-only lookup. Traversal, symlink
substitution, corrupt content, malformed indexes, and digest mismatch fail
closed; corrupt entries are removed rather than rendered. Restart reconstructs
the bounded verified inventory without network or render-thread I/O.

## Verification

- BigBoy `.130`, slot `func017-offline-cache-s5-r1`: 16/16 focused Maps tests
  passed, including eight new authority tests and a 64-step quota/restart
  property trace.
- Scoped rustfmt and owned-file `git diff --check` passed.
- The completed disposable slot was removed; only reproducible build output was
  deleted.

## Remaining acceptance gap

Daemon download orchestration, trusted production catalog-digest delivery,
basemap handoff, package ownership policy, and package/live offline-region proof
remain. FUNC-017 stays `Remaining`.
