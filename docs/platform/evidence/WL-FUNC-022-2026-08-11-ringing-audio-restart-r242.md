# WL-FUNC-022 ringing-audio restart recovery — 2026-08-11

- Scope: Clock startup reasserts an acknowledged but still-ringing audio Start
  after daemon restart. The request receives a fresh restart-time TTL while
  retaining its deterministic effect ID, so a surviving Music daemon
  deduplicates it and a restarted Music daemon restores playback.
- Farm: `172.20.0.90`.
- Focused gate: `install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::clock::tests::restart_reasserts_acknowledged_ringing_audio_with_same_effect_id -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed, 4,793 filtered out.
