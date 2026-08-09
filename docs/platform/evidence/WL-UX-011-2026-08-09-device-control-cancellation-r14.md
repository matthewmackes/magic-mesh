# WL-UX-011 — fail-closed pending device-control cancellation

Date: 2026-08-09

## Implemented

- The Device Manager retains the exact dispatched request, renders a pending-operation banner, and offers **Cancel pending**.
- While that exact request is retained, a second device mutation is refused and no second queue record is written, so dispatch cannot replace the identity being polled or cancelled.
- Cancellation is a typed, exact-body-authorized envelope with a unique cancellation id and the original request id, operation, device identity, provider host, provider generation, and requesting seat.
- Cancellation ids and host/request path components are bounded and character-allowlisted; cancellation has no command or free-form execution-path field.
- The provider authorizes the cancellation before atomically renaming and removing the exact queued request. That rename is the cancellation linearization point: once the ordinary executor has claimed an operation, cancellation returns `NotPending` under the cancellation id and cannot report the original operation as cancelled.
- Terminal results carry a typed `Succeeded`, `Failed`, `Cancelled`, or `NotPending` outcome. Every cancellation attempt is appended to the existing hash-chain admin-action audit plane.
- Rejected or late cancellation results use the cancellation id, preventing an unsigned or stale cancellation marker from replacing the authoritative execution result for the original request.
- Pending UI state retains the cancellation request id independently. It polls both ids: `Cancelled` under the original id terminates the operation, while `NotPending`/`Failed` under the cancellation id surfaces a warning, clears only the cancellation-in-flight display, and continues waiting for the original terminal result.
- All newly public cancellation-envelope fields and terminal-outcome variants have API documentation; the initial `missing_docs` warnings for this API were corrected.

## Focused BigBoy verification

Host: `172.20.0.130` (BigBoy)

Slot: `ux011-device-cancel-r1`

- `cargo test -p mackes-mesh-types device_control::tests::cancellation_claim_is_exact_and_only_succeeds_while_pending -- --exact --nocapture`
  - PASS: 1 passed, 0 failed, 492 filtered out.
- `cargo test -p mackesd --lib --features async-services workers::device_control::tests::signed_exact_cancellation_claims_only_a_still_pending_request_and_is_audited -- --exact --nocapture`
  - PASS: 1 passed, 0 failed, 4384 filtered out.
- `cargo test -p mde-shell-egui device_manager::tests::dispatch_to_a_fresh_host_writes_the_request_to_the_targets_replicated_dir -- --exact --nocapture`
  - PASS: 1 passed, 0 failed, 1499 filtered out.
- `cargo test -p mde-shell-egui device_manager::tests::pending_control_cancel_is_signed_and_bound_to_the_exact_dispatched_request -- --exact --nocapture`
  - PASS: 1 passed, 0 failed, 1500 filtered out.
- `cargo test -p mde-shell-egui device_manager::tests::a_second_device_mutation_cannot_replace_the_retained_pending_identity -- --exact --nocapture`
  - PASS: 1 passed, 0 failed, 1502 filtered out.
- `cargo test -p mde-shell-egui device_manager::tests::cancellation_refusal_clears_only_its_id_then_original_result_terminates_pending -- --exact --nocapture`
  - PASS: 1 passed, 0 failed, 1502 filtered out.
- Focused total recorded here: 6 tests passed, 0 failed across six exact invocations.
- Changed Rust files were formatted individually with farm-host `rustfmt --edition 2021`.

## Remaining boundary

- Cancellation is intentionally pending-only. Once the provider has claimed an operation for execution, there is no safe generic rollback for sysfs writes or fixed hardware commands; the UI waits for and reports the original typed terminal result.
- No broad suite was run. Existing unrelated workspace warnings remain outside this lane; no new `missing_docs` warning remains on the cancellation API.
