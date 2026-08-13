# WL-FUNC-022 S5 retained Clock action consumption (r511)

Date: 2026-08-13

## Result

Notification Center now consumes the exact retained Clock occurrence/schedule
action payload before publishing a Snooze, Stop, or Add-one-minute request.
The retained row fails closed after the first activation, and an unchanged live
daemon projection cannot re-arm that consumed authority. A mismatched payload
or generation is never published.

This closes a shell lifecycle gap where a retained Clock row could publish the
same typed action on successive frames while waiting for the daemon projection
to converge. Scheduling, signing, audio, and due-state authority remain outside
the render path.

## Scope

- `crates/desktop/mde-shell-egui/src/notification_center.rs`
- This evidence record

Concurrent storage, VDI, worker, and device-control edits were preserved and
excluded.

## Farm evidence

- `.50`, slot `func022-nc-action2`:
  `cargo test -p mde-shell-egui retained_clock_action_is_consumed_before_publication_and_cannot_rearm -- --nocapture`
  passed 1/1.
- `.170`, slot `func022-nc-clippy`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings` passed.
- `.50`, slot `func022-nc-fmt`: file-scoped
  `rustfmt --edition 2021 --check` passed.
- `git diff --check` passed.

The initial focused-test attempt on BigBoy `.130` was interrupted by unrelated
farm contention and is excluded. The exact gate was rerouted once to `.50`; no
duplicate test lane was retained.

## Remaining acceptance

First-release package integration remains, followed by the explicitly deferred,
non-blocking installed-seat proof for ordinary and focused-VDI banner actions,
bell/history lifecycle, direct-DRM chrome, physical audio, restart/suspend, and
selected-peer convergence.
