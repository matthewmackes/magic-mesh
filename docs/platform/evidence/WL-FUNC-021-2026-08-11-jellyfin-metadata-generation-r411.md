# WL-FUNC-021 Jellyfin metadata generation — 2026-08-11

- Scope: persisted Jellyfin metadata must remain bound to the newest admitted provider generation.
- Hostile boundary: older generations and conflicting same-generation snapshots cannot replace current metadata; byte-identical same-generation replay remains idempotent.
- Focused gate: `cargo test -p mde-jellyfin cache::tests::metadata_replay_cannot_replace_current_provider_generation -- --exact --nocapture`.
- Farm: `172.20.0.130`, slot 3, admitted with 16,186,924 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 99 filtered out.
- Remaining boundary: live provider rotation/replay during outage through the installed UI remains.
