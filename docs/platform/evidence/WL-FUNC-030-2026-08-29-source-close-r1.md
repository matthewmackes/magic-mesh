# WL-FUNC-030 source close — 2026-08-29

Classification: source/cargo close. Live Bus set/get/clear leftover
remains `WL-TEST-003` after a testing Beta.

Tree: `5f9685408` plus the voice-admin persist compile fix (dirty).
`production_admitted: false`. No dest invented. No credentials invented.

## Why this closes

S1 is in-tree: Activity SIP gateway form publishes set/get/clear,
never echoes the password, and renders present/absent honestly.
Malformed hosts refuse. The voip responder and `gateway.toml` contract
are unchanged.

## Farm

Reused `cargo test -p mde-collab-egui` job `bdec5d40433e` (203/0).
No second grind.
