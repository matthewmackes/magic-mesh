# WL-FUNC-021 — source-aware typed workspace playback (2026-08-06)

Status: implementation slice complete; WL-FUNC-021 remains `Remaining` because
live two-catalog audible playback, network-loss acceptance, target/DLNA proof,
GUI-worker removal, and direct-DRM proof are still open.

## Invariant

The daemon remains the sole Music queue and native-engine authority. A typed
workspace `play` request may select only a source variant already present in
the retained catalog and matching the current queue entry. The production
workspace poller passes the bounded admitted client set; it does not create a
provider or playback authority in the UI. If the selected source is unavailable,
the daemon may use that variant's verified finite cache, otherwise it refuses
with a stable `source_unavailable` result.

## Implementation

- `crates/services/mde-musicd/src/bus_responder.rs` now passes the complete
  bounded client set into the typed workspace action lane while keeping
  provider playlist/curation mutations on the primary writer.
- A selected source variant is resolved through the retained catalog, pinned
  as the first candidate for the current logical queue track, and sent through
  the existing `Engine::play_from_candidates` path. Remaining queue entries
  retain the normal source fallback ordering.
- A selected unreachable variant uses the existing finite cache only when the
  cache index proves the exact song bytes; unknown or unadmitted source
  identities fail closed. No queue, Bus, or engine authority was duplicated.

## Farm verification

- `.50` focused `cargo test -p mde-musicd
  typed_play_selection_uses_requested_admitted_source_variant -- --nocapture`:
  **1 passed, 0 failed**.
- BigBoy `.130` full `cargo test -p mde-musicd -- --nocapture`: **158 passed,
  0 failed**; doctests: **0 passed, 0 failed**.
- `.90` package-scoped `cargo fmt -p mde-musicd -- --check`: passed.
- Local `git diff --check` for the touched responder: passed.
- A full-workspace formatter invocation reports unrelated pre-existing drift in
  the shared dirty tree; no unrelated files were reformatted or rewritten.

Source SHA-256:

```text
2ad3c05c8edc8a3f47415a6254adae5b377f076bb1f6328b857ff5fc57a3748c  crates/services/mde-musicd/src/bus_responder.rs
```

## Open acceptance

This evidence proves the typed daemon integration and hostile source rejection,
not live audio. Two admitted catalogs, audible network-loss playback, position-
continuous seat handoff, admitted DLNA control, complete GUI-worker migration,
and direct-DRM/live hardware proof remain required before WL-FUNC-021 or the
overall drain goal can close.
