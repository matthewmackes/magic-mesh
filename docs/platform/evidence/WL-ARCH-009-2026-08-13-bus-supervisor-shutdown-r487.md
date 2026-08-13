# WL-ARCH-009 — Bus supervisor shutdown ownership

Date: 2026-08-13

## Scope

`BusSupervisor` previously waited through its complete child-restart delay after
`mde-bus` exited. The singleton-owner path waits 30 seconds, so a process-group
shutdown arriving during that interval could not complete until the retry timer
expired. The missing-binary retry had a separate interruptible wait.

The supervisor now routes both retry paths through one shutdown-aware wait. A
shutdown edge ends the worker without another child activation, including while
the 30-second singleton-owner backoff is active. The regression uses Tokio's
paused clock to prove shutdown wins without advancing the retry timer.

## Farm gates

- Host `172.20.0.130` (BigBoy), slot
  `arch009-bus-shutdown-focused-r486`:
  `cargo test -p mackesd --locked singleton_owner_backoff_is_interrupted_by_group_shutdown --lib -- --nocapture`
  passed `1/1` (`0` failed; `4926` filtered out).
- Host `172.20.0.170`, slot `arch009-bus-supervisor-module-r486`:
  `cargo test -p mackesd --locked workers::bus_supervisor::tests --lib -- --nocapture`
  passed `4/4` (`0` failed; `4923` filtered out).
- Host `172.20.0.196`, slot `arch009-bus-supervisor-clippy-r486`:
  `cargo clippy -p mackesd --locked --lib -- -D warnings` completed with exit
  code `0`.

## Remaining epic acceptance

This closes one bounded process-supervision shutdown gap. `WL-ARCH-009` remains
open for the broader ownership and unified Workers UI cutover, removal of
duplicate runtime surfaces, and the deferred package/fleet/live proof described
by the canonical worklist.
