# WL-UX-013 — health history and duration UI slice (2026-08-05)

This record covers one implementation slice of the canonical health modal. It
does not close WL-UX-013 or claim live expected-state/recovery acceptance.

## Implemented

- `crates/desktop/mde-shell-egui/src/health_modal.rs` now renders an explicit
  `Active Issues` section before `Recent History`.
- Resolved history is bounded to eight rows and ordered by severity, then
  longest elapsed resolution duration, then stable issue ID.
- Elapsed durations use readable sub-hour units, `HH:MM:SS` at one hour, and
  `<days>d HH:MM:SS` at and beyond one day; they are never wall-clock values.

## Verification

BigBoy `.130`, slot `wl-health-ui-r1`:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=wl-health-ui-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui health_modal -- --nocapture
```

Result: `10 passed; 0 failed; 1438 filtered out`. The test also wrote the
existing Dark, Light, narrow, and large-text rendered proof images. The exact
farm slot was removed after verification.

The remaining shared expected-state wire contract, lifecycle publishers,
recurrence/detail/export depth, and physical fleet transition proof remain
open.
