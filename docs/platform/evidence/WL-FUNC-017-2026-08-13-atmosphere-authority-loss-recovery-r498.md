# WL-FUNC-017 atmospheric authority-loss recovery — 2026-08-13

- Scope: `crates/mesh/mackesd/src/workers/weather_atmosphere.rs`.
- Gap closed: when the effective weather-location authority became unavailable
  or invalid, the atmospheric worker previously retried while leaving the last
  typed temperature, wind, and cloud imagery published for the old location.
- Behavior: authority loss now replaces a still-current typed atmospheric
  snapshot with an explicit `location_authority_unavailable` reset. Repeated
  recovery polls are idempotent and do not flood the Bus. Once a valid new
  location generation and provider response return, the worker publishes fresh
  imagery for that exact recovered authority.
- Focused hostile gate: `.170`, slot
  `func017-atmosphere-authority-test-r498` —
  `cargo test -p mackesd --lib --features async-services workers::weather_atmosphere::tests::location_authority_loss_revokes_old_imagery_and_recovery_republishes -- --exact --nocapture`;
  **PASS**, 1 passed, 0 failed, 4,947 filtered out.
- Static gate: BigBoy `.130`, slot
  `func017-atmosphere-authority-clippy-r498` —
  `cargo clippy -p mackesd --lib --features async-services -- -D warnings`;
  **PASS**.
- Formatting gate: `.170`, slot
  `func017-atmosphere-authority-fmt-r498` —
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/weather_atmosphere.rs`;
  **PASS**. The initial helper invocation was rejected before execution because
  `xcp-build.sh` accepts Cargo subcommands only; the final check ran directly in
  the helper-synced isolated farm workspace.
- Farm conditions: `.90` was occupied by unrelated work, `.50` was below the
  8-GiB sync floor, and `.196` was unreachable. No duplicate or filler gate was
  launched.
- Remaining acceptance: provisioned offline map/weather data, responsive Maps
  and launcher integration, and the deferred post-release NWS/nowCOAST,
  provider-loss/restart, sleep/rejoin, package-upgrade, and installed-seat proof.
