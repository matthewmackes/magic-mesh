# WL-FUNC-021 party election key — 2026-08-11

- Scope: party playback remembers the complete deterministic election key
  `(sequence, issued time, origin seat)` for the applied command.
- Hostile boundary: a later-arriving deterministic winner at the same sequence
  replaces an already-applied losing authority instead of being ignored.
- Focused gate: `cargo test -p mde-media-core party::tests::later_same_sequence_winner_replaces_already_applied_party_authority -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 2, admitted with 9.65 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 266 filtered out.
- Remaining boundary: live multi-seat party playback and package proof remain.
