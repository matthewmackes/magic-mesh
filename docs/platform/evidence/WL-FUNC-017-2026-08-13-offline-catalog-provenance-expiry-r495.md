# WL-FUNC-017 offline catalog provenance and expiry boundary — r495

## Result

The reachable Maps offline-catalog authority now rejects ambiguous provisioned
region revisions before they can become catalog provenance. Revision identities
must be bounded ASCII tokens, begin and end with an alphanumeric byte, and may
otherwise contain only alphanumerics plus `-`, `_`, `.`, and `+`. Leading or
trailing separators, whitespace, path separators, control characters, and
Unicode lookalikes fail closed while the whole admitted document remains bound
to its caller-provided SHA-256.

Tile authorization now treats `expires_at_ms` as an exclusive deadline. A tile
is permitted immediately before expiry and revoked at the exact expiry instant,
so a provisioned catalog cannot retain one extra clock tick of offline authority.
The existing digest-bound catalog admission remains the atomic document-update
boundary; this slice does not create a second cache or updater.

## Farm verification

- BigBoy `.130`, slot `func017-offline-catalog-boundary-test-r495`: focused
  `offline_catalog::tests::` passed 4/4, including ambiguous provenance refusal,
  exact-deadline revocation, digest admission, and coordinate bounds.
- BigBoy `.130`, slot `func017-offline-catalog-boundary-clippy-r495`:
  `cargo clippy -p mde-maps-location-egui --all-targets -- -D warnings` passed.
- `.196`, slot `func017-offline-catalog-boundary-filefmt-r495`:
  `rustfmt --edition 2021 --check
  crates/desktop/mde-maps-location-egui/src/offline_catalog.rs` passed after the
  host reported 8,537,940 KiB free, above the 8-GiB sync floor.

The first exact-test invocation used an unqualified test name and selected zero
tests; it was rejected as evidence. The corrected qualified invocation passed
1/1 before the 4/4 module gate. An initial package-wide fmt check on `.50`
reported unrelated pre-existing `offline_cache.rs` drift plus one owned-file
wrap; the owned wrap was corrected and the required file-scoped `.196` gate
passed. No unrelated source was changed.

## Remaining acceptance

`WL-FUNC-017` still requires provisioned offline/provider data and package/live
proof, complete offline route/weather behavior, MG90 hardware recovery, and the
deferred post-release Maps/Car acceptance matrix.
