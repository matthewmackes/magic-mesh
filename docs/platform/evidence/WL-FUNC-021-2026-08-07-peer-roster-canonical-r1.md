# WL-FUNC-021 S5 peer-roster canonical identity audit (2026-08-07)

## Finding

The cross-seat handoff target admission path consumed every `*.json` file
under `music-state-by-peer/` and trusted the embedded `MusicState.peer` value.
It did not verify that the filename was the canonical snapshot path for that
peer. A stale, copied, or renamed snapshot could therefore advertise
`seat-15` while residing at `seat-16.json`; `playback_targets` and the typed
`transfer` action would treat the embedded identity as an admitted target even
though that seat did not publish the snapshot.

## Production correction

`crates/services/mde-musicd/src/state.rs:139-143` now defines
`canonical_peer_state_path`, requiring the filename to equal
`<embedded-peer>.json`. `read_all_peer_states` applies that check after the
existing bounded decode and record validation (`state.rs:382-399`). A valid
canonical snapshot remains admitted; a mismatched filename is ignored.

## Focused farm verification

Farm lane: `172.20.0.50`, slot
`music-s5-peer-canonical-r1b`.

```text
cargo test -p mde-musicd peer_state -- --nocapture
running 3 tests
test state::tests::peer_state_reader_rejects_snapshot_with_mismatched_peer_filename ... ok
test state::tests::read_all_peer_states_collects_and_sorts_snapshots ... ok
test state::tests::peer_state_reader_keeps_newest_bounded_backlog ... ok
test result: ok. 3 passed; 0 failed; 180 filtered out
```

The existing handoff regression set also passed on the same farm lane:

```text
cargo test -p mde-musicd handoff -- --nocapture
test result: ok. 7 passed; 0 failed; 176 filtered out
```

The integrated state/CLI source was included in a Fedora 44 container RPM cut
on BigBoy (`MCNF_BUILD_SLOT=music-f44-container-rpm-state-cli-r1`). Release 5
payload gates passed: base 83.5 MiB and lighthouse 11.9 MiB. Pulled artifact
SHA-256 values are `1c152ca995c7ee29f88a50166da78a89704f66b2596796cff081b28439bb52fd`
and `2662512194a72905e2f4d97381cf3402fa92e96d081e45e41d4694bf7683c3fb`.

## Remaining live proof

This proves deterministic target-roster admission and the existing handoff
unit behavior, but not a two-seat runtime transfer. Dell remains unavailable,
so the required live proof is still: publish fresh idle/playing heartbeats on
two seats, execute an authorized transfer, verify exactly one owner, preserve
the song and position, and confirm the target resumes without a duplicate
completion.
