# WL-FUNC-021 Airsonic track identity — 2026-08-11

- Scope: `get_song(id)` requires the provider response to carry the exact
  requested song identity.
- Hostile boundary: a provider cannot substitute another track and artwork row
  beneath the requested lookup key.
- Focused gate: `cargo test -p mde-musicd airsonic::tests::get_song_rejects_provider_substitution_of_requested_track_identity -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 2, admitted with 12.5 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 252 filtered out.
- Remaining boundary: live provider substitution/outage proof remains.
