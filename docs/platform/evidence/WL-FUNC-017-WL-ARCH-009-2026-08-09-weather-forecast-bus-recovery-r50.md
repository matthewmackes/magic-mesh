# WL-FUNC-017 / WL-ARCH-009 weather-forecast Bus recovery r50 — 2026-08-09

## Behavioral proof

`WeatherForecastWorker` no longer freezes a missing construction-time Bus root.
Every authority read and post-provider publication transaction resolves the
current explicit/user root, uses `mde_bus::SYSTEM_BUS_ROOT` when that resolver is
absent, and fresh-opens `Persist`. The same worker therefore recovers after a
late Bus becomes openable and follows a Bus directory/index replacement. The
exact effective-location identity is still re-read from that fresh Bus after
provider fetch and before projection publication; a changed generation/point/
timezone/coverage discards both results.

For every requested current/forecast refresh, both snapshots are now completely
derived, contract-validated, and JSON-serialized in memory before the first Bus
write. An injected forecast serialization failure proved that neither current
nor forecast was published and no cache was persisted. Fresh outcomes return
only after every requested Bus write and required fresh cache persistence
succeeds; an injected cache persistence failure returned an error rather than
fresh success.

Residual caveat: the two topic writes remain sequential because `Persist` has no
multi-topic atomic batch here. They are staging-atomic against derivation and
serialization failures, but not crash-atomic: a process/host crash or second
write failure after the first commit can expose a partial current/forecast pair.

## Farm verification

Host: machine 193, `172.20.0.90`
Slot: `weather-forecast-bus-r50`

The clean detached verification worktree invoked the repository farm helper with
explicit host and slot. No local Cargo command was run.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=weather-forecast-bus-r50 /tmp/magic-mesh-weather-r50/install-helpers/xcp-build.sh cargo test -q -p mackesd --features async-services --lib workers::weather_forecast::tests::late_and_replaced_bus_are_reopened_for_current_transactions -- --exact --nocapture
# PASS: 1 passed; 0 failed; 4497 filtered out

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=weather-forecast-bus-r50 /tmp/magic-mesh-weather-r50/install-helpers/xcp-build.sh cargo test -q -p mackesd --features async-services --lib workers::weather_forecast::tests::stages_requested_pair_and_requires_fresh_cache_commit -- --exact --nocapture
# PASS: 1 passed; 0 failed; 4497 filtered out

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=weather-forecast-bus-r50 /tmp/magic-mesh-weather-r50/install-helpers/xcp-build.sh cargo test -q -p mackesd --features async-services --lib workers::weather_forecast::tests::effective_generation_change_during_fetch_discards_both_projections -- --exact --nocapture
# PASS: 1 passed; 0 failed; 4497 filtered out

printf '%s\n' 'cd ~/magic-mesh-farm-weather-forecast-bus-r50' 'rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/weather_forecast.rs' 'sha256sum crates/mesh/mackesd/src/workers/weather_forecast.rs' 'exit' | MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=weather-forecast-bus-r50 /tmp/magic-mesh-weather-r50/install-helpers/xcp-build.sh shell
# PASS; remote source hash printed below

git diff --check -- crates/mesh/mackesd/src/workers/weather_forecast.rs docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-weather-forecast-bus-recovery-r50.md
# PASS
```

An initial package-wide `cargo fmt -p mackesd -- --check` also reported unrelated
format drift in files outside this slice. Those files were preserved; the owned
source passed the exact single-file farm rustfmt command above.

## Source hash

```text
54d4b432cc1b975715acd504e27ee0f9413c471099acc534f3992930734b0374  crates/mesh/mackesd/src/workers/weather_forecast.rs
```

The local and machine-193 source hashes matched. `WORKLIST.md` was not edited;
no commit or push was made.
