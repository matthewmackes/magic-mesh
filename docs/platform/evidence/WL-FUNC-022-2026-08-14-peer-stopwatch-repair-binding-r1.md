# WL-FUNC-022 — peer stopwatch repair binding

- Date: 2026-08-14
- Revision: `8f61d8af4bc3ea6e5b8d778c8fa86bd2c268486c`
- Farm: BigBoy `172.20.0.130`, slot `clock-audit`
- Command: `cargo test -p mackesd workers::clock --lib`
- Result: 36 passed, 0 failed, 4,963 filtered

The hostile approved-peer stopwatch fixture initially found that a lower-
revision payload with an arbitrary request id could overwrite the admitted
stopwatch while reusing the current local snapshot revision. Peer repair now
requires the deterministic origin-generated request identity bound to the
target, stopwatch identity, and repaired revision. The focused hostile test
and the full Clock worker suite pass after the fix.

This proves the Clock worker's peer-repair admission boundary. Installed-seat,
package, and physical-audio acceptance remain owned by `WL-TEST-001` and are
not required to close the Clock implementation epic.
