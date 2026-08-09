# WL-FUNC-017 S6 — Navigation action retry boundary (r7)

Date: 2026-08-09

Base revision: `add2ba63`

## Boundary corrected

The navigation worker previously advanced a Bus action cursor before its
governed state writes and publications succeeded. A failed publication of the
intermediate `Calculating` snapshot durably retained both the advanced cursor
and replay reservation. Restart then converted the snapshot to interrupted at
a newer generation, so the original route action could never be retried.

The action cursor is now the final acknowledgement boundary: route, progress,
and cancellation effects run first, then the cursor and final snapshot commit.
Recovery of an in-flight calculation rolls back its one-generation replay
reservation. The still-unacknowledged request can therefore retry at its
original expected generation, while a final state that committed before a Bus
publication failure remains acknowledged and is republished after restart.

The hostile regression replaces the navigation state topic directory with a
regular file, proves the first calculation publication fails without advancing
the route cursor, repairs the path, simulates restart, and proves the same Bus
request reaches one active route at generation 1.

## Focused farm verification

Host: machine 193, `172.20.0.90`

Slot: `navigation-retry-r20`

Command:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=navigation-retry-r20 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::navigation::tests::failed_calculating_publication_keeps_route_action_retryable -- --exact --nocapture
```

Result after the final source sync:

```text
running 1 test
test workers::navigation::tests::failed_calculating_publication_keeps_route_action_retryable ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4398 filtered out
```

The exact source file also passed farm `rustfmt --check`. No broad suite or live
seat action was run.

## Remaining scope

This closes one S6 action/restart boundary. A production route provider,
offline route calculation, responsive Maps/Car presentation, and live route
proof remain open under WL-FUNC-017.

