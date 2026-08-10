# WL-FUNC-021 cast media URL admission — 2026-08-10 r184

## Correction

`NetworkCaster` now refuses a cast request before discovery/renderer admission
when its media locator is not a bounded `http://` or `https://` URL with a
valid nonzero port (when specified). Local paths, alternate schemes,
authority-less URLs, credentials, whitespace, and malformed ports cannot be
forwarded into `SetAVTransportURI`.

## Farm proof

- Host: `172.20.0.130` (BigBoy)
- Slot: `func021-cast-media-url-r184b`
- Command: `cargo test -p mde-media-core cast::tests::non_network_or_malformed_media_urls_fail_before_cast_admission -- --nocapture`
- Result: `1 passed; 0 failed; 261 filtered out`
- Source SHA-256: `e2b09aab899d7ae81edcf8be12bb6b77bb1f84a9596d25f0bfbb3525c2038aa7`
- Orchestrator `git diff --check`: passed

The package-wide `cargo fmt --check` remains affected by pre-existing
formatting drift in `crates/desktop/mde-media-core/src/roaming.rs`; this
checkpoint changes only `cast.rs` and the new test is formatted locally.
No live renderer was available for this admission-boundary correction, so
physical DLNA/Chromecast cast proof remains the existing WL-FUNC-021 blocker.
