# WL-FUNC-028 source close — 2026-08-29

Classification: source/cargo close. Live Construct Transfers leftover
remains `WL-TEST-003` after a testing Beta.

Source revision: `5f9685408` on `agent/drain-worklist-20260725`.
`production_admitted: false`. No dest invented. No seat mutation.

## Why this closes

S1–S2 are in-tree: `mackesd transfer sync-pair add|remove|list` posts
`TransferVerb::{SaveSyncPair, RemoveSyncPair}`; the Transfers editor
publishes the same verbs and mirrors worker next-run / last-result /
unreachable. Malformed intervals and unknown ids refuse. No second
store or scheduler.

## Farm (current HEAD, not re-run)

| command | job | ended | result |
|---|---|---|---|
| `cargo test -p mackesd` | `56644bb14a6c` | 2026-08-29T00:40:32Z | pass |

Do not grind `mde-collab-egui` for this close; that crate is red at
HEAD from an unrelated voice-admin move, already fixed in the dirty
tree.

Live leftover: `WL-FUNC-028-2026-08-26-cli-gui-list-parity-r1.md`.
