# WL-FUNC-021 queue atomic persistence — 2026-08-09 r1

## Correction

`mde-musicd` previously rewrote its authoritative `music-queue.json` in place. A
failed or interrupted write could therefore expose a truncated file which the
reader interpreted as an empty queue. Queue persistence now writes and syncs a
unique sibling, atomically renames it over the prior snapshot, syncs the parent
directory, and removes a failed temporary file. The old complete queue remains
authoritative when replacement fails.

## Farm proof

- Host: `172.20.0.50`
- Slot: `func021-queue-atomic-r1-20260809`
- Focused hostile regression: `1 passed, 0 failed`
- Complete queue module: `14 passed, 0 failed`
- Exact-file `rustfmt --check`: passed
- Scoped `git diff --check`: passed on the orchestrator
- Source SHA-256: `d857fd97085a72904d1c30ed9bd3764325d7137699ad53c174b0f3d7191ca822`

This is deterministic persistence proof, not live audible playback, provider
loss, renderer, cast, or physical two-seat handoff proof.
