# WL-FUNC-021 audible provider-loss tail handoff — 2026-08-11

- Scope: an audibly authoritative live source drains its valid decoded tail after provider loss and hands off at the retained enqueue boundary.
- Hostile boundary: provider loss cannot replay the current track, cut valid queued audio, or grant byte-zero fallback authority.
- Focused gate: `cargo test -p mde-musicd engine::tests::audible_live_provider_loss_preserves_queued_tail_until_track_handoff -- --exact --nocapture`.
- Farm: BigBoy `172.20.0.130`, slot 2, admitted with 14,428,200 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 261 filtered out.
- Remaining boundary: physical CPAL gapless behavior during a real provider disconnect remains.
