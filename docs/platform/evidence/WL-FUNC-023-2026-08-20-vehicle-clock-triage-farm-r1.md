# WL-FUNC-023 vehicle publication clock triage — 2026-08-20

## Root cause

`workers::vehicle::tests::gateway_change_and_heartbeat_publication_clocks_are_isolated`
was exposing a deterministic comparison bug, not a scheduler race. Each
`build_state_v2` fold derives `VehicleDomainFreshness.age_ms` from the current
wall clock. `VehicleRosterSnapshot::content_eq` already ignored sequence and
publication/observation timestamps, but compared those derived age values. A
metadata-only refresh could therefore be classified as `Changed`, producing
two publications where only gateway B's telemetry change was real. The failure
was timing-sensitive because the derived ages vary between folds.

The fix keeps freshness state and reasons in semantic comparison but clears
the five derived `age_ms` values before comparing snapshots. This preserves
the independent source publication and heartbeat clocks without suppressing
reported telemetry changes.

## Farm verification

Admission and execution:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=2 \
  install-helpers/xcp-build.sh cargo test -p mackesd \
  gateway_change_and_heartbeat_publication_clocks_are_isolated -- --nocapture
```

- Host: XEN-BIGBOY `172.20.0.130`, slot `2`
- Admission: `16,625,232 KiB` free; required `8,388,608 KiB`
- Result: `1 passed, 0 failed, 0 ignored, 5029 filtered out`
- Exit: `0`
- Elapsed: `538.1s`

The farm emitted the pre-existing unused-method warning for
`workers/call_media.rs`; that file was not changed.

## Capacity

The focused job was admitted on a non-ENOSPC BigBoy lane. At dispatch,
`.170` had only `155M` free and remained inadmissible for cargo work; other
lanes were occupied or had less headroom. No capacity failure occurred for
this verification.
