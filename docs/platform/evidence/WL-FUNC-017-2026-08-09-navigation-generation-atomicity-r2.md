# WL-FUNC-017 navigation generation atomicity — 2026-08-09

The daemon navigation authority now checks that the next generation exists
before replacing its retained progress. At `u64::MAX`, a valid progress action
therefore leaves the entire last-good snapshot unchanged instead of publishing
new progress under an exhausted generation.

No maneuver, remaining-distance, or remaining-duration monotonic policy was
added: those values may legitimately move backward after off-route movement,
route matching, or traffic correction. Existing timestamp, route identity,
schema, host, replay, and generation admission remain authoritative.

Source: `crates/mesh/mackesd/src/workers/navigation.rs`, SHA-256
`905d116b9e10a5e163802ae697e116bdfb9fdc716e8b8633b09ac199fa1424ac`.

## Verification

- Farm host `172.20.0.50`, slot
  `func017-navigation-monotonic-r2-20260809`.
- `cargo test -p mackesd --features async-services --lib
  workers::navigation::tests -- --nocapture`: 4 passed, 0 failed, including
  `exhausted_generation_preserves_last_good_progress_atomically`.
- Exact-file `rustfmt --edition 2021 --check` and changed-file
  `git diff --check` passed after formatting.
- A broader cargo invocation also attempted non-test targets and encountered
  the pre-existing cloud-export compile failure; the scoped library test target
  above compiled and passed without changing that unrelated surface.

WL-FUNC-017 remains `Remaining`; live routing-provider, offline dataset, and
online/offline/reconnect acceptance are outside this checkpoint.
