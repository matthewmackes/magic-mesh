# WL-FUNC-017 first-release offline catalog materializer — r527

## Result

First-release assembly can now consume the immutable bundle emitted by
`packaging/maps/produce-offline-catalog.py` and publish a complete private Maps
cache root with one no-replace rename. The materializer performs no network
I/O and makes no release-promotion claim.

Before creating its staging directory it requires:

- an immutable real-directory bundle and singly linked, read-only inputs;
- exact release source revision, source epoch, and quota matches;
- manifest-bound catalog, cache-index, and tile SHA-256 values;
- an exact one-to-one manifest/index tile set with bounded sizes and quota;
- safe generated payload paths with no traversal, symlink, hardlink, or
  writable-directory authority; and
- successful admission by the existing Rust verifier, which uses production
  `VerifiedCatalog` and `OfflineTileCache` contracts.

The output uses private `0700` directories and `0600` regular files. Catalog,
schema-2 index, and all payloads are written and fsynced below a uniquely owned
stage; the cache root becomes visible only through the final atomic rename.
Existing cache roots are never replaced. Pre-publication failures remove the
stage and leave no cache root.

## Files

- `packaging/maps/materialize-offline-catalog.py`
- `packaging/maps/materialize-offline-catalog-test.py`

No producer, Rust runtime, release collector, or worklist file was changed.

## Farm evidence

- `.50`, slot `func017-maps-materializer-hostile-r527`:
  `python3 packaging/maps/materialize-offline-catalog-test.py` passed. The suite
  covered success, exact revision/epoch/quota binding, catalog mutation,
  writable bundle authority, hardlinks, symlinks, verifier refusal, no-replace,
  and absence of partial output.
- `.170`, slot `func017-maps-materializer-static-r527`:
  `python3 -m py_compile packaging/maps/materialize-offline-catalog.py
  packaging/maps/materialize-offline-catalog-test.py` and `python3 -m tabnanny
  ...` passed.
- BigBoy `.130`, slot `func017-maps-materializer-integration-r527`:
  `CARGO_INCREMENTAL=0 cargo build --manifest-path
  packaging/maps/verifier/Cargo.toml` passed. A read-only, single-link staging
  copy of that verifier then ran through
  `MAPS_MATERIALIZER_VERIFIER=... python3
  packaging/maps/materialize-offline-catalog-test.py`; the complete governed
  producer/materializer and production Rust cache-contract integration passed.
- Local orchestration-only checks: Python compilation and `git diff --check`
  passed before the farm wave; the final diff check passed before commit.

## Remaining acceptance

- Approve and supply the actual first-release region/tile set and data license.
- Produce the immutable bundle for the exact first-release revision and epoch.
- Run this materializer during first-release assembly with the release-built
  verifier, and include the resulting cache identity in release evidence.
- Verify the assembled package/image in the first full release.
- After release, perform the deferred non-blocking one-node offline
  Maps/navigation/weather/MG90 acceptance matrix.
