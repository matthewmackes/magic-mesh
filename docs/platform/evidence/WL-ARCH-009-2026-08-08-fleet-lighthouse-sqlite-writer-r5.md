# WL-ARCH-009 fleet/lighthouse SQLite writer cutover — 2026-08-08

Fleet setting pushes and lighthouse CA enrollment now cross bounded typed
operations into the process-isolated SQLite writer. Fleet revision plus
per-peer apply rows commit atomically and exact retries do not duplicate them.
Lighthouse CA seed admission validates identity, epoch, certificate size, and
conflicting durable state before an atomic write. The remaining direct-write
baseline fell from 31 to 26 reviewed sites.

## Verification

BigBoy `.130`, slot `arch009-fleet-nebula-writer-r1`:

- Writer tests: 8/8 passed.
- Fleet tests: 19/19 passed.
- Nebula IPC focused test: 1/1 passed.
- SQLite authority self-test and actual lint passed with 26 residual sites.
- Scoped rustfmt and `git diff --check` passed.

## Remaining acceptance gap

Twenty-six allowlisted direct-write sites and live six-group writer failure and
restart proof remain, so ARCH-009 stays `Remaining`.
