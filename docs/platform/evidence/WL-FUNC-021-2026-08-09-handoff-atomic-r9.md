# WL-FUNC-021 handoff record atomic persistence — r9

Date: 2026-08-09

Revision inspected: `7deff274f93c86f144d813aa680e54b96e3b7f72` plus this uncommitted lane.

## Production change

`mde-musicd` now persists playback authority, per-peer snapshots, takeover
intents, and handoff completions through uniquely named sibling files. Each
complete JSON record is synced, atomically renamed over its target, and followed
by a parent-directory sync. A failed replacement removes its temporary file and
preserves the last-good record.

The authoritative `music-state.json` is committed before its derived per-peer
snapshot. The two files are independently atomic, not a filesystem transaction:
if snapshot publication fails, roster readers remain safely stale rather than
observing state that the local owner never committed. Intent and completion
readers admit only final `.json` records, so temporary siblings cannot authorize
a handoff.

## Focused farm verification

Machine 194 (`172.20.0.170`), slot `func021-handoff-atomic-r9`:

```text
cargo test -p mde-musicd state::tests -- --nocapture
19 passed; 0 failed; 217 filtered out
```

The failure-injection regression
`state::tests::failed_handoff_record_replace_preserves_last_good_and_cleans_temporary`
proved last-good completion preservation and temporary-file cleanup. Exact-file
`rustfmt --edition 2024 --check crates/services/mde-musicd/src/state.rs` and a
scoped `git diff --check` passed. Source SHA-256:
`451e6b3e998649e25882268f8c53c4a5b42df0368ef750ce02784c45beb7d784`.

This checkpoint does not claim cross-file transactions, live two-seat handoff,
physical renderer, provider-loss continuity, cast hardware, package, commit, or
push proof.
