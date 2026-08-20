# WL-FUNC-023 lifecycle progress acknowledgement evidence — 2026-08-20

## Scope

This slice adds the missing typed checkpoint boundary in
`crates/mesh/mde-enroll/src/lifecycle_controller.rs`. Renderer clients now
accept authority-owned `LifecycleProgressV1` acknowledgements only when the
request, generation, and target are bound to the controller. Per-target
checkpoints are monotonic; exact duplicates are refused as replay, regressions
and changed totals are refused, and terminal checkpoints cannot be advanced.
The last non-terminal checkpoint is available for interruption resume. No
renderer mutation authority was added.

## Farm verification

Admission and execution:

```text
MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mde-enroll \
  lifecycle_controller -- --nocapture
```

- Host: XEN-196 `172.20.0.196`, slot `1`
- Admission: `14,302,132 KiB` free; required `8,388,608 KiB`
- Source revision at dispatch: `0c0d1e43684b897a1a5493b901ab2b62bfbb18fe`
- Result: `5 passed, 0 failed, 0 ignored, 35 filtered out`
- Exit: `0`
- Elapsed: `14.1s`

The tests cover typed acknowledgement and interruption resume, replay,
generation/scope refusal, monotonic regression refusal, terminal refusal, and
independent checkpoints for multiple fleet targets. The farm emitted one
pre-existing unused-method warning in `workers/call_media.rs`; that file is
outside this slice and was not changed.

## Boundary and blockers

This is farm contract evidence only. It does not claim live Bus delivery,
provider execution, SSH enrollment, installed first boot, or physical-seat
acceptance. The existing dirty worktree contains unrelated collaboration
changes and an unrelated `WL-FUNC-032` evidence file; they were preserved.
