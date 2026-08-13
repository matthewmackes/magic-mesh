# WL-ARCH-010 — Workload request correlation (r486)

Date: 2026-08-13

## Acceptance gap

The Workloads shell retained only placement node and workload identity after
publishing a typed operation. A previous terminal projection for the same
workload could therefore settle a newer request before its own generation was
published. The shell also ignored the Workload contract's correlated
`reply/<message-ulid>` refusal, leaving rejected requests presented as waiting
for readiness.

## Implementation

- Pending UI state now retains the Bus message ULID, request id, placement,
  workload id, and bounded wait start.
- Projection resolution requires the exact request id; a retained older
  generation cannot complete a newer operation.
- Correlated Workload replies reject duplicate JSON keys, foreign request ids,
  invalid accepted/refused shapes, and invalid projected status before UI use.
- Typed refusals settle immediately as failed audit entries with their stable
  refusal code. Missing status remains bounded by the existing request timeout.

Source hashes at the gate revision:

- `crates/desktop/mde-shell-egui/src/iac/mod.rs`:
  `630ff53b397922a10b3e054351acb1235ba4b95a8e38d96e57c2f8ec0d45fd7f`
- `crates/desktop/mde-shell-egui/src/workload_api.rs`:
  `b3607217d85e0c5ce4b743f9de8a561e1d8c0136ae97c7635770563b95c79aeb`

## Farm gates

All authoritative reruns used a detached clean worktree at `f66c1b39` with only
this slice applied, because concurrent out-of-scope shell changes did not
compile and were explicitly excluded from this task.

- Host `.196`, slot `arch010-request-correlation-test-r486`:
  `cargo test -p mde-shell-egui workload_api::tests -- --nocapture`
  passed 8/8, with 1,576 filtered out. The matrix includes stale terminal
  projection refusal and exact correlated RPC-refusal decoding.
- Host `.196`, the same exact synced slot after the test completed:
  `cargo fmt -p mde-shell-egui -- --check` passed.
- Host `.50`, slot `arch010-request-correlation-clippy-r486`:
  the unmodified strict all-target/all-feature invocation reached the package
  and found only existing out-of-scope lints in `communications/mod.rs`,
  `car_keymap.rs`, `status_bar.rs`, and `system/mesh.rs`. The slice gate then
  passed with `-D warnings` and only those four exact lint names suppressed:
  `clippy::while-let-loop`, `clippy::manual-string-new`,
  `clippy::drop-non-drop`, and `clippy::items-after-test-module`.

No live-seat claim is made. Remaining epic acceptance is the post-release
native KMS/Display1 lifecycle and recovery matrix plus the repository-wide
strict-Clippy baseline already named by the active worklist.
