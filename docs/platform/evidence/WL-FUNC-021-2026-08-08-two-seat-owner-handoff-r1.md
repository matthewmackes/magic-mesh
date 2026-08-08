# WL-FUNC-021 — exact two-seat playback ownership handoff

Status: code-level owner-yield/target-resume boundary complete; physical
two-seat acceptance remains `Remaining`.

## Behavior

- The yielding seat transfers the complete bounded admitted queue and exact
  cursor/playhead instead of asking the target to infer identity from a song ID.
- A durable one-use intent suppresses repeated source yields. The target
  revalidates the intent and bounded lease immediately before starting audio.
- Consumed, superseded, expired, aliased, and queue-mismatched transfers cannot
  acquire playback authority.
- Target state-persistence failure revokes target audio and preserves the
  completion for recovery. If no target commits before expiry, the unchanged
  source queue reclaims authority; a committed target heartbeat prevents a
  delayed cleanup from causing split-brain.
- Target source resolution uses the seat-local admitted catalog, not the mesh
  coordination root.

## Farm verification

Host `.50`, slot `func021-two-seat-handoff-r1`:

```text
cargo test -p mde-musicd two_seat_handoff_is_exact_once_replay_safe_and_failure_honest -- --nocapture
1 passed; 0 failed; 208 filtered out
```

Host `.90`, slot `func021-handoff-state-r1`:

```text
cargo test -p mde-musicd state::tests -- --nocapture
18 passed; 0 failed; 191 filtered out
```

Review gate on `.90`, slot `func021-handoff-review-r2`, repeated the focused
test after adding the hostile idle-target case; it passed 1/1. An idle target
projection is not accepted as proof that audible authority transferred.

`install-helpers/lint-worklist.sh --self-test`, the canonical worklist lint,
and scoped `git diff --check` pass. The package-wide format check remains
blocked by concurrent pre-existing rustfmt drift outside this handoff slice;
no unrelated files were reformatted.

## Source hashes

```text
e46c6fd432ae91e4442c89e7adcc919e66a513a813c93e6f4244ff8c1a48ba2a  crates/services/mde-musicd/src/state.rs
141ecd375bdb082e0f1e8a36115db363ca76cbb0aa337ae89809c7455038309f  crates/services/mde-musicd/src/bus_responder.rs
```

## Hardware blockers

No physical second admitted mesh seat was available for an audible transfer.
Live proof still requires source-seat playback, target-seat resume at the same
queue/playhead, stale-transfer injection, and captured output/daemon traces on
both seats. DLNA and Chromecast hardware acceptance remain separate FUNC-021
blockers.
