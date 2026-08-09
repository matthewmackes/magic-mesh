# Weather Location Bus truth and recovery checkpoint (R47)

Date: 2026-08-09
Worklist: `WL-FUNC-017`, `WL-ARCH-009`
Base commit: `55df5cce39bebce729a390c5fb3a963e0e47d76f`

## Runtime semantics

`WeatherLocationWorker` now resolves the configured/user Bus root to a concrete
path with `mde_bus::SYSTEM_BUS_ROOT` as the canonical fallback. Durable weather
authority loads independently before Bus startup. The same supervised worker
survives unresolved and unopenable startup with shutdown-aware exponential
backoff bounded from 10 ms to 2 s. It retains one mutable `Persist`, refreshes it
with `reopen_if_index_changed()` before each pass, and defers runtime Bus
transaction failures with the same bounded behavior instead of exiting.

Fix acquisition is now `Result<Option<LiveLocationFix>>`. A pass first stages
the complete weather action lane, then performs complete vehicle-topic
discovery. Every exact admitted `state/vehicle/<same-host>/<source-id>` lane is
read successfully before any candidate fix is parsed or selected. Discovery or
any admitted lane-read failure rejects the entire pass; an unavailable lane can
never masquerade as no fix and cause a false fallback/unavailable projection.
Malformed or semantically invalid individual vehicle bodies remain terminally
ignored only after the complete Bus view has been read.

Both the staged action lane and complete vehicle-fix view exist before any
authority mutation, durable action cursor, fix reconciliation, projection
clear, or Bus publication. This preserves retained action replay while making
partial input views effect-free.

The existing corrected-forward publication contract remains intact. Accepted
authority and its cursor are durably stored first. Projection-clear or location
publication failure leaves `last_published_generation` and
`last_published_at_ms` unchanged, so the same worker repairs the unpublished
generation on its next complete pass without replaying the accepted action.
State-store failure leaves in-memory authority and cursor unchanged.

## Focused farm verification

Host: machine9 (`172.20.0.50`)
Slot: `weather-location-bus-r47`

The following exact affected tests ran:

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::weather_location::tests::weather_location_bus_root_falls_back_to_canonical_system_spool \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::weather_location::tests::production_fix_reader_rejects_stale_wrong_host_and_unsupported_coverage \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::weather_location::tests::final_vehicle_lane_read_failure_defers_action_and_all_projection_changes \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::weather_location::tests::failed_projection_publication_repairs_forward_without_false_publish_marker \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::weather_location::tests::late_and_replaced_bus_recovers_external_action_and_shutdown \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::weather_location::tests::atomic_state_rejects_symlink_and_failed_persistence_does_not_admit_action \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::weather_location::tests::auto_prefers_fresh_fix_then_saved_verified_fallback_and_recovers_restart \
  -- --exact --nocapture
```

Each command passed: `1 passed; 0 failed; 4,489 filtered out`.

The final-lane hostile test stages a weather action, successfully reads the
first admitted vehicle lane, fails the final admitted lane, and observes no
in-memory or durable authority change, no cursor advance, no publication-marker
change, and no output rows. The corrected-forward test forces projection
publication failure after durable action admission, verifies publication
markers remain on the prior generation, and proves the next pass repairs the
new generation. The async recovery test keeps one worker alive across unresolved
and permission-denied opens, unlinks and recreates the SQLite index, publishes a
manual action through the replacement handle, observes generation 2 from the
same worker, and proves prompt shutdown. Existing tests retain valid-fix,
saved-fallback, restart, malformed-fix, and state-store failure semantics.

Farm formatting passed:

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/weather_location.rs
```

The first compile was blocked before this module by an unrelated concurrent
`health_reconciler.rs` edit missing its retry helper. Only that file and other
unrelated dirty worker files were restored to `HEAD` in the ephemeral R47 farm
copy. No unrelated local file was changed; the isolated compile and exact tests
then passed.

Source SHA-256:
`ed2aaaca7e29a5e6d8a1167ee12c1bc0021287f58edaecffda1c37ad1c550452`.

## Scope

No broad suite, package build, live-seat proof, WORKLIST edit, commit, push, or
unrelated test was run. This checkpoint is limited to weather-location Bus-root
recovery, complete action/fix input staging, replacement-index visibility,
corrected-forward publication repair, and preservation of durable authority
semantics.
