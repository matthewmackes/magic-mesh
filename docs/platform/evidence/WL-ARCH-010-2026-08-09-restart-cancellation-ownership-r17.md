# WL-ARCH-010 S3/S4 — restart cancellation ownership

Date: 2026-08-09

## Production correction

The sole `mackesd` Workload reconciler now treats every nonterminal cancellation
as the exclusive owner of its exact target's next adapter effect. A restart
target journaled in `Stopping` is therefore not independently observed into
`Starting` while cancellation cleanup is waiting on durable backoff.

An accepted cancellation also continues target cleanup after its client-facing
deadline. It no longer enters generic operation expiry, which could invoke
cleanup against the cancellation request itself and strand the target. The
existing bounded cancellation retry budget remains authoritative; no second
actuator, lifecycle API, or UI state was added.

## Hostile recovery proof

The test seeds a restart at the durable `Starting` boundary, accepts a
cancellation whose first target cleanup remains `Stopping`, closes the journal,
then reopens it at the durable retry after the cancellation deadline. It proves:

- the restart target receives zero independent `observe` calls;
- both cleanup effects receive the original `Restart` target, never `Cancel`;
- the target reaches `Cancelled`; and
- the cancellation reaches `Completed`.

## Focused BigBoy verification

Host: BigBoy (`172.20.0.130`), slot `arch010-cancel`.

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-cancel \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  --features async-services \
  reopened_expired_cancel_owns_restart_target_until_cleanup_completes \
  -- --nocapture
```

Result: **1 passed, 0 failed, 4,390 filtered out**.

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-cancel \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  --features async-services \
  reopened_starting_restart_counts_the_only_start_effect_and_advances_journal \
  -- --nocapture
```

Result: **1 passed, 0 failed, 4,390 filtered out**.

The crate emitted pre-existing warnings outside this ownership slice. Scoped
`git diff --check` passed. Local `rustfmt` was unavailable; both farm tests
compiled the changed production and test code successfully.

## Blockers

None for this boundary.
