# WL-FUNC-022 stopwatch conflict repair — r493

Date: 2026-08-13

## Executable gap

The Clock origin already detected a peer mirror carrying a newer conflicting
stopwatch payload and published its authoritative payload as a repair. The
receiver nevertheless discarded every such repair solely because the origin's
stopwatch revision was lower. The existing regression stopped after inspecting
the outbound command, so it did not prove convergence.

`ClockWorker` now admits that lower-revision origin repair only when the signed
command is bound to the receiver's exact current snapshot generation. The
origin payload can therefore replace a conflicting mirror, while a delayed
repair fails closed after any intervening receiver mutation and leaves the
newer state unchanged. Same-revision conflicts remain rejected.

The strengthened regression proves both application of the current-generation
repair and rejection of a stale-generation rollback.

## Farm gates

- `172.20.0.170`, slot `func022-stopwatch-repair-test-r493b`:
  `cargo test -p mackesd workers::clock::tests::peer_stopwatch_convergence_repairs_newer_conflict_without_stale_rollback -- --exact --nocapture`
  passed 1/1 with 4,933 library tests filtered.
- `172.20.0.130`, slot `func022-stopwatch-repair-clippy-r493`:
  `cargo clippy -p mackesd --lib -- -D warnings` passed.
- `172.20.0.90`, slot `func022-stopwatch-repair-fmt-r493b`: Rustfmt's
  normalized output was compared over the touched production and regression
  ranges; both ranges were clean. A whole-file probe identified three existing
  formatting differences outside this slice, which were deliberately not
  swept into the commit.

The initial `.90` test lane was stopped after it queued behind an unrelated
package-cache/filesystem lock. `.196` safely refused sync at 5.4 GiB free, below
the 8 GiB floor. The test was moved to free `.170`; neither condition weakened
the final gates.

## Remaining epic acceptance

The full post-release Clock acceptance still requires installed package and
multi-seat proof for World Clock, alarms, timers, stopwatch mirroring,
offline/rejoin convergence, audio actions, clock/bell routing, lock curtain,
and one authoritative daemon-owned schedule across the selected physical
nodes.
