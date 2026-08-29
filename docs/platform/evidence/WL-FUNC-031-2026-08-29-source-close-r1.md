# WL-FUNC-031 source close — 2026-08-29

Classification: source/cargo close. Live two-seat co-edit leftover
remains `WL-TEST-003` after a testing Beta.

Tree: `5f9685408` plus the voice-admin persist compile fix (dirty).
`production_admitted: false`. No dest invented.

## Why this closes

S1–S2 are in-tree: Documents mode mounts `live_document_share_session()`
with Share/Join/Follow/Close. Non-members and closed sessions refuse.
Phase-3c markers are gone from `documents.rs`. Two-instance fixture
coverage is in the crate suite.

## Farm

Reused `cargo test -p mde-collab-egui` job `bdec5d40433e` (203/0).
No second grind.
