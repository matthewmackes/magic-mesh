# WL-FUNC-020 Android launch UI wire slice — 2026-08-05

This evidence records the Workloads-side launch interaction only. It does not
claim a live Android guest, Cuttlefish boot, package-manager success, or VDI
session readiness.

## Implemented slice

- `crates/desktop/mde-shell-egui/src/iac/android_apps.rs` returns a selection
  only for a fresh, validated, launch-ready inventory entry and carries the
  admitted workload identity plus placement host.
- `crates/desktop/mde-shell-egui/src/iac/mod.rs` opens the existing review gate,
  freezes a closed `android_app_launch` body, mints the short-lived
  `exec-request` capability, and publishes to `action/exec/request`.
- The wire body contains only schema version, typed action kind, target host,
  workload identity, and closed AOSP app enum. It contains no command or raw
  Android intent fields.
- The daemon/provider remains authoritative for guest launch outcome; provider
  unavailability is reported as failure rather than success.

## Farm verification

Command on farm `.170`, slot `wl-android-launch-ui-r1`:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=wl-android-launch-ui-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui \
  iac::tests::android_starter_launch_uses_the_audited_typed_exec_lane -- --nocapture
```

Result: `1 passed; 0 failed; 0 ignored; 0 measured; 1448 filtered out`.

The test verifies the review-gated request, exact `action/exec/request` topic,
closed `android_app_launch` payload, absence of command/intent escape fields,
and capability binding to `exec-request`, `fleet-control`, and the canonical
node/workload/package target. The temporary farm workspace was removed after
the run.

## Remaining proof

Real Cuttlefish provider registration, guest boot/ADB/package evidence, session
and inner-display attach, focused input/audio/clipboard policy, reconnect, and
live per-app frame evidence remain open under WL-FUNC-020.
