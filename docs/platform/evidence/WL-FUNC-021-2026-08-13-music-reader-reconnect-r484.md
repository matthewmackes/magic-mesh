# WL-FUNC-021 Music workspace reader reconnect evidence

Date: 2026-08-13

## Scope

The desktop Music workspace reader now drops its long-lived Bus handle when a
retained workspace row is malformed, contract-invalid, equivocated, or stale.
The next poll can therefore reopen a repaired/replaced Bus index instead of
reusing a poisoned connection and indefinitely presenting provider loss as an
empty workspace. The change is limited to
`crates/desktop/mde-music-egui/src/workspace_reader.rs`.

## Farm gates

- BigBoy `.130`, slot
  `func021-music-reader-reconnect-test-20260813`:
  `cargo test -p mde-music-egui --locked workspace_reader::tests::reader_reopens_after_malformed_retained_row_and_accepts_repaired_snapshot --lib -- --nocapture`
  — **PASS**, 1 passed, 0 failed.
- `.90`, slot `func021-music-reader-reconnect-clippy-20260813`:
  `cargo clippy -p mde-music-egui --locked --lib` — **PASS** (no errors).
- `.50`, slot `func021-music-reader-reconnect-fmt-20260813`:
  `cargo fmt -p mde-music-egui -- --check` — **BLOCKED by pre-existing
  formatting drift** in `crates/desktop/mde-music-egui/src/main.rs`; that file
  was not changed by this slice.

## Acceptance remaining

WL-FUNC-021 still requires renderer/audio/cast/handoff implementation and
post-release live second-seat/provider proof. This slice does not claim those
criteria.
