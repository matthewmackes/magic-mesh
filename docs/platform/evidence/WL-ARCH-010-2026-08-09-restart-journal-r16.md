# WL-ARCH-010 restart journal recovery — r16

Date: 2026-08-09

## Outcome

Workload Restart no longer invokes a monolithic backend restart effect. The
sole actuator issues an idempotent shutdown/stop, persists Stopping, observes
the backend stopped, then persists Starting before issuing start. Recovery of
a durable Starting phase observes an already-running backend without repeating
start; a still-stopped backend receives the start and advances to
WaitingForGuest. VM and Quadlet backends retain their fixed command adapters.

The Workload phase contract now explicitly admits Stopping to Starting, and
the ledger advances that transition directly. Restart also releases any local
Display1 attachment when stop begins.

## Focused farm verification

Host: machine 194 (172.20.0.170)

Slot: arch010-restart-journal-r16c

Command: cargo test -p mackesd --lib --features async-services
workers::workload_compute::tests::reopened_starting_restart_counts_the_only_start_effect_and_advances_journal
-- --exact --nocapture

Result: 1 passed, 0 failed, 4,383 filtered out. The test reopens the real
journal at Starting, proves zero starts when the backend is already running,
exactly one start when it is stopped, durable advancement to WaitingForGuest,
and no repeated start after a second journal reopen/reconcile.

A prior pure sequencing test passed during development but is not acceptance
evidence; the reopened-ledger effect-count test above is the success-critical
proof.
