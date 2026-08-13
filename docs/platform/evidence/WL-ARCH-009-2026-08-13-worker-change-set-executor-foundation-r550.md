# WL-ARCH-009 — Worker change-set executor foundation (r550)

Date: 2026-08-13

## Production change

- The canonical worker registry now owns an explicit typed action-descriptor
  seam. Every existing row intentionally declares no mutation rather than
  manufacturing an endpoint.
- `WorkerChangeSetExecutor` admits only an exact declared worker/action and
  supervisor generation, retains a bounded immutable preview, binds Commit and
  Cancel to its exact target/items/digest/deadline, and rejects request replay.
- With no authenticated provider handler registered, Commit returns `Refused`;
  it never publishes placeholder success or invokes a direct provider bypass.

## Exact verification

- `git diff --check` over the owned files: passed.
- Farm command on `.50`, slot 2:
  `cargo test -p mackesd --features async-services worker_change_set::tests:: -- --nocapture`
  compiled the new production module and reached final test-binary linkage.
  It emitted two slice warnings; both were corrected exactly in the owned file.
  The link remained active without test execution for more than eight minutes
  under concurrent lane contention and was stopped by operator finish-now
  direction. It is **not** recorded as a passing test and there is no nonzero
  discovery claim.
- No broader equivalent, Clippy, build, or formatting rerun was started after
  the finish-now direction.

## Recorded debt

- Run the module-qualified hostile tests to nonzero completion on a provisioned
  farm lane against the final corrected source.
- Run strict relevant Clippy, build, and Rustfmt once against that source.
- Wire a Bus consumer through exact-body `ActionAuthorizer` before this executor
  can consume untrusted Bus requests in production.
- Add the first canonical descriptor and authenticated mutation handler only
  when an existing worker's supervisor-generation-aware mutation semantics can
  be reused without a bypass.
