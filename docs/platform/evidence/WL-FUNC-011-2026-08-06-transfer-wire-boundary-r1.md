# WL-FUNC-011 transfer wire boundary — 2026-08-06

`crates/shared/mde-collab-types/src/tests.rs` adds a hostile media transfer
fixture proving duplicate `schema_version` JSON fields are rejected by
`TransferJobV2::from_json` rather than ambiguously admitted.

Verification: Big farm `.50`, slot `collab-transfer-duplicate-20260806-r3`,
focused cargo test passed 1/1; `.90` was not used after its disk filled during
an independent sync retry. Source SHA-256:
`7be9ab4a1d6075efcac8c42ae42474836542749e67efd34fde7da8136bbcd25a`.

Cross-node collaboration, provider, and live media proof remain open; Dell
was not modified.
