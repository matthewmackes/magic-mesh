# WL-FUNC-016 S1 rich clipboard contract — 2026-08-08

The canonical signed `mde-collab-types` clipboard V2 envelope now exposes a
bounded offer/selection/payload contract for plain text, HTML, PNG/JPEG images,
opaque Files lists, and finite typed metadata. A selection is bound to the exact
clip, source session, generation, MIME kind, and payload digest. Origin, target,
expiry, payload limits, and replay state remain part of signed admission.

The wire denial vocabulary is finite: unknown version, oversized, stale,
secret-bearing, unsupported, replayed, and invalid payload. Unknown fields and
MIME values fail serde admission; secret-classified offers remain auditable but
cannot be selected or materialized. Default metadata/disclosure fields are
omitted so existing V2 default-offer signing bytes remain compatible.

## Farm verification

Host `.50`, slot `func016-s1-rich-v2-r1`:

- `cargo test -p mde-collab-types -- --nocapture`: **72 passed, 0 failed**;
  doc tests **0 failed**.
- Changed-file Rust formatting on the same synced slot:
  `rustfmt --edition 2021 --check .../clipboard_v2.rs` and
  `rustfmt --edition 2021 --check --config skip_children=true .../lib.rs` both
  exited 0.
- Operational contract tests added:
  `rich_offer_selection_and_payload_contract_round_trips_all_required_kinds`,
  `hostile_unknown_selection_version_and_mime_fail_closed`,
  `hostile_oversized_payload_and_metadata_fail_with_typed_denial`,
  `hostile_stale_and_replayed_selections_fail_with_distinct_typed_denials`,
  `hostile_secret_bearing_offer_cannot_be_selected_or_materialized`, and
  `hostile_unsupported_payload_cannot_be_selected_as_success`.

## Recorded hashes and fixtures

- Contract source SHA-256 (`clipboard_v2.rs`):
  `03ff63b7b2d89cdf8f6c9b2b11c4249f7be31e4c053c104d95f986d7e60c84c8`.
- Public export source SHA-256 (`lib.rs`):
  `bad0b77ff902e719022bf33a577e4e8891a765d1b0916615e186ebf8ffd2a510`.
- Deterministic signed-envelope fixture SHA-256:
  `4b970f57631ebdfa9e850c362e600e2403e4b1f02d956e4740df145224569fc1`.
- Deterministic accepted-selection fixture SHA-256:
  `771917de2d79ccf4b2131b3812cb00ad1b499427f397f9fc42d1b2078c63c31f`.
- `rich_contract_fixture_hashes_are_stable` asserts both fixture hashes.

## Honest remaining gaps

This proves the shared S1 contract, not live transport. Local DRM ownership,
authenticated mesh deduplication/cleanup, VDI guest protocol adapters, UI
permissions, and five-seat proof remain S2-S5. The older
`mackes-mesh-types::vdi_clipboard` envelope is still consumed by existing
adapters; migrating those adapters to this canonical signed contract belongs to
their implementation slices and was intentionally not claimed here. The crate's
broad strict-clippy gate also has pre-existing warnings outside this scoped
contract; no clean all-target clippy claim is made.
