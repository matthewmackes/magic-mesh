# WL-FUNC-021 — offline cache content digest (r514)

Date: 2026-08-13

## Result

`mde-musicd` now records the SHA-256 identity of each completely written audio
cache object. Both the lightweight suffix admission probe and the byte-returning
offline read recompute that identity before granting playback authority. A
same-length replacement therefore fails closed after restart and cannot refresh
LRU state. Legacy index entries without a digest remain visible to cache
bookkeeping but are not admitted as verified offline audio.

The cache reuses the crate's existing, tested action-auth SHA-256 implementation;
the helper received only crate-local visibility. No second digest implementation
or new dependency was introduced.

## Farm gates

- `.90`, slot `func021-cache-digest-test`: `cargo test -p mde-musicd
  same_length_cached_track_replacement_is_not_admitted_after_restart --
  --nocapture` — passed 1/1, 271 filtered out.
- `.196`, slot `func021-cache-digest-clippy`: `cargo clippy -p mde-musicd
  --lib -- -D warnings` — passed.
- `.170`, slot `func021-cache-digest-fmt`: file-scoped Rustfmt for `cache.rs` —
  passed. Crate-wide formatting was not claimed because `bus_responder.rs`
  contains unrelated pre-existing formatting drift; the visibility-only hunk is
  formatting-neutral.
- `git diff --check` — passed.

BigBoy was not used. The concurrent System, worker, hardware-probe, taskbar, and
curtain scopes were preserved.

## Remaining acceptance

First-release packaging remains, followed by the user-deferred non-blocking
installed-seat proof for offline playback, cache replacement/corruption,
provider loss/switching, restart, and audible continuity.
