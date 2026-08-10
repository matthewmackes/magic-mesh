# WL-FUNC-022 — peer command budget evidence

- Date: 2026-08-10
- Farm host: `172.20.0.130` (BigBoy)
- Farm slot: `func022-peer-command-bound-r152`
- Gate: `cargo test -p mackesd --lib workers::clock::tests::peer_convergence_is_bounded_per_tick -- --nocapture`
- Result: 1 passed, 0 failed

Clock peer schedule, stopwatch, and acknowledgement convergence is capped at
128 Bus commands per tick. Remaining convergence is retried on a later tick,
keeping a large peer roster from monopolizing the scheduler.
