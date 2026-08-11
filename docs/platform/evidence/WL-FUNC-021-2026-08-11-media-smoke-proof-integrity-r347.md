# WL-FUNC-021 media smoke proof integrity — 2026-08-11

- Scope: real-clip smoke success requires observed `Playing`, resolved audio
  output, and a nonblank decoded frame within 30 seconds.
- Hostile boundary: terminal states and timeout without the complete observation
  set fail instead of manufacturing `smoke OK`.
- Focused gate: `cargo test -p mde-media-core --bin media_smoke tests::terminal_or_timeout_without_decode_proof_cannot_report_smoke_success -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 2, admitted with 10.7 GiB free.
- Result: **PASS**, 1 passed, 0 failed.
- Remaining boundary: real installed-player clip proof remains.
