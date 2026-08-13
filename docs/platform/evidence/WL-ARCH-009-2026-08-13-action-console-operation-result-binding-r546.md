# WL-ARCH-009 — Action Console operation/result binding (r546)

Date: 2026-08-13

## Implemented result

The Workers Action Console now records the exact Preview, Commit, or Cancel
operation currently awaiting a result, together with its successful Bus
publication time. A result is admitted only when it matches that operation and
was completed no earlier than publication. Result item identities are also
confined to the immutable staged change set.

This closes an authority/replay gap caused by all three protocol phases sharing
one request ID: a delayed Preview result can no longer answer a later Commit or
Cancel, clear its pending state, or be presented as its terminal outcome.

## Hostile regression

`workbench::action_console::tests::delayed_preview_result_cannot_answer_commit_or_cancel`
publishes and admits a valid Preview, publishes Commit, then substitutes a
later-delivered Preview result with the same request, target, and generation.
The console keeps Commit pending and retains the already-admitted Preview as
Preview rather than treating the replay as Commit completion.

## Farm gates

- `.196` slot 1: the corrected fully-qualified hostile regression passed 1/1
  (`cargo test -p mde-shell-egui
  workbench::action_console::tests::delayed_preview_result_cannot_answer_commit_or_cancel
  -- --exact --nocapture`). The first BigBoy selector compiled current source
  but selected 0 tests because the unqualified name was incompatible with
  `--exact`; it is not counted as evidence.
- `.196` slot 1: `cargo build -p mde-shell-egui --all-targets --all-features`
  passed.
- `.196` slot 1: `cargo fmt -p mde-shell-egui -- --check` passed.
- BigBoy slot 3: strict `cargo clippy -p mde-shell-egui --all-targets
  --all-features -- -D warnings` reached the shell and stopped only on the
  pre-existing, concurrently owned `communications/mod.rs:608`
  `clippy::while_let_loop` finding. It was not rerun or modified.
- Scoped `git diff --check` passed.

## Remaining ARCH-009 acceptance

- Finish the S3 provider/action ownership inventory and remove any remaining
  generic or duplicate node-management authority.
- Complete S5 filters, responsive/largest-text render evidence, and the final
  action-bypass audit.
- Complete S6 Network Operations projections and legacy route/help/package
  cutover.
- Build the first full release, then run the deferred non-blocking one-node
  process isolation, staged-change/partial-failure, recovery, and live capture
  acceptance. Additional nodes remain optional.
