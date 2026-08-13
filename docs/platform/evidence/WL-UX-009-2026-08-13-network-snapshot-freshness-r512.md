# WL-UX-009 — Network snapshot freshness semantics (r512)

## Implemented result

The Construct Network surface now treats the producer's `generated_ms` as the
authority for live semantic state. The 30-second producer may miss two updates,
but after 90 seconds its retained topology is explicitly marked stale. Missing
or materially future-dated timestamps are unavailable. In every non-fresh
state the last topology remains visible for diagnosis while live leader, peer,
link-count, and service-health tones are revoked.

This closes a concrete UX-009 stale/unavailable semantic gap. It adds no Network
mutation, no render-path I/O, and no synthetic topology.

## Farm verification

- Focused hostile regression, `.90`, slot `ux009-network-test2`:
  `cargo test -p mde-shell-egui stale_missing_and_future_snapshots_lose_live_semantics -- --nocapture`
- Strict production Clippy, `.90`, warmed slot `ux009-network-test2`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings`
- File-scoped Rustfmt, `.50`:
  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/network.rs`

## Remaining acceptance

WL-UX-009 still requires complete Construct surface migration, first-release
payload verification, and the deferred post-release Dark/Light, narrow,
largest-text, stale/unavailable, motion, focus, and direct-DRM human review.
