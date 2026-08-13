# WL-FUNC-017 governed offline catalog producer — r525

Date: 2026-08-13

## Result

The first-release Maps input path now has a canonical producer for an already
approved offline tile set. The producer performs no network access and does not
invent or acquire map content. It requires immutable, singly-linked regular
approval and tile files, re-attests each file identity while reading, and
rejects unsafe or overlapping source paths, duplicate tile identities, invalid
coordinates, unsupported provider identity, malformed geographic bounds,
expired regions, digest mismatch, and quota overflow.

Publication is atomic and no-replace. The result contains:

- the exact schema-1 `catalog.json` accepted by Maps `VerifiedCatalog`;
- a content-addressed `payload/<region>/<z>/<x>/<y>-<sha256>.tile` tree and
  schema-2 `payload/index.json` accepted by `OfflineTileCache`;
- an immutable release manifest binding provider attribution and license,
  geographic bounds and zooms, region revision/expiry, exact source revision
  and epoch, quota and aggregate payload size, runtime-catalog/index digests,
  and every tile's path, digest, and size.

The runtime catalog deliberately cannot carry the additional provenance fields
because its existing `deny_unknown_fields` schema is intentionally narrow. The
release manifest binds those fields without weakening or duplicating the Maps
runtime verifier. No Maps Rust production file was changed.

## Owned files

- `packaging/maps/produce-offline-catalog.py`
- `packaging/maps/test-produce-offline-catalog.py`
- `packaging/maps/verifier/Cargo.toml`
- `packaging/maps/verifier/src/main.rs`

## Exact farm gates

- `.50`, slot `func017-offline-catalog-test-r525`:
  `python3 packaging/maps/test-produce-offline-catalog.py` passed. The hostile
  suite covered duplicate identities, overlapping source paths, traversal,
  digest mismatch, quota overflow, hard links, writable inputs, symlinked
  parents, and no-replace publication.
- `.170`, slot `func017-offline-catalog-py-r525`:
  `python3 -m py_compile` and `python3 -m tabnanny` passed for both Python files.
- BigBoy `.130`, slot `func017-offline-catalog-rust-r525`:
  built the isolated verifier and ran the producer hostile suite with
  `MAPS_CATALOG_VERIFIER` pointing at it. The verifier used the production
  `VerifiedCatalog::admit_json`, `OfflineTileCache::open`, and
  `OfflineTileCache::lookup` APIs; all produced tiles were accepted with exact
  digest/length binding.
- `.196`, slot `func017-offline-catalog-fmt-r525`:
  `cargo fmt --manifest-path packaging/maps/verifier/Cargo.toml -- --check`
  passed after applying the formatter output.
- `.90`, slot `func017-offline-catalog-clippy-r525`:
  `cargo clippy --manifest-path packaging/maps/verifier/Cargo.toml -- -D warnings`
  passed. A duplicate invocation blocked on the same target lock was terminated;
  only the retained unique gate result is claimed.
- Local `git diff --check -- packaging/maps` passed.

No paid/unapproved map data, release build, download, placeholder release tile,
or live-seat proof was produced.

## Remaining FUNC-017 inputs

- Select and approve the real bounded first-release region/tile set and its
  provider attribution/license under the applicable data terms.
- Make the approved bytes immutable and run this producer for the exact release
  Git revision/epoch, quota, and expiry policy.
- Materialize the immutable bundle into the writable Maps cache root during
  release assembly and verify its manifest/catalog/index in the first full
  release build.
- After release, perform the deferred non-blocking one-seat offline
  Maps/navigation/provider-loss/restart/sleep-rejoin/MG90/weather/visual proof.
