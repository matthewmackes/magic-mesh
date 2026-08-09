# WL-UX-014 F-grade backlog bound — 2026-08-09 r1

## Production correction

`ToastHost` previously retained an unbounded `VecDeque` behind the visible
lower third. Because grade F health alerts hold until acknowledgement, an absent
operator or failed producer could grow the shell process indefinitely.

The one canonical queue now retains at most 64 waiting alerts. At saturation a
Critical may replace the newest non-critical waiter; non-critical overflow and
Critical overflow into an all-Critical backlog are rejected. The visible and
already-admitted acknowledgement-required alerts keep FIFO order.

## Farm proof

- Host: BigBoy `172.20.0.130`
- Slot: `ux014-kiron-fgrade-backlog-r1-20260809`
- Focused regression: `1 passed; 0 failed`
- Complete `toast::tests` suite: `34 passed; 0 failed`
- Exact-file `rustfmt --edition 2021 --check`: passed
- `toast.rs` SHA-256:
  `be0162a399e41a26348eaac97237533c48ef303e7ea0da4df5e12b01914e1114`

This slice bounds S1 queue behavior only. It does not claim the A-F scene,
ticker, renderer-fallback, audio, package, or live-seat acceptance work.
