# WL-FUNC-029 source close — 2026-08-29

Classification: source/cargo close. Live Vitelity leftover remains
`WL-TEST-003` after a testing Beta.

Tree: `5f9685408` plus the voice-admin persist compile fix (dirty).
`production_admitted: false`. No dest invented. No credentials invented.

## Why this closes

S1 is in-tree: Activity Fleet voice panel publishes provision, DID
route, failover, shared-outbound, and cutover through the existing
verbs and renders `state/voice` projections, including an honest empty
unprovisioned state. Invalid DIDs, unknown nodes, and conflicting
routes refuse at the verb boundary. The persist-after-section fix keeps
the provision notice across frames without moving `form` twice.

## Farm

Reused `cargo test -p mde-collab-egui` job `bdec5d40433e` (203/0).
No second grind.
