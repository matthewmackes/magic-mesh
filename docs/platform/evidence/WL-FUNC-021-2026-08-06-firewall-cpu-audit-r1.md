# WL-FUNC-021 — firewall monitor common-seat CPU audit

Date: 2026-08-06
Scope: source audit of `crates/mesh/mackesd/src/workers/firewall_monitor.rs`.

## Finding

The worker was a credible common-seat CPU contributor, although this source
audit does not claim live attribution. Every five-second tick ran synchronous
work on the Tokio worker: it forked `journalctl --version`, forked a second
`journalctl` to read the kernel journal, read/wrote the cursor file, and
rewrote and reparsed the complete seven-day firewall JSONL file even when no
new denial existed. The retention rewrite was especially avoidable: the
worker's own comment called it cheap, but it was only cheap when the file was
absent.

## Source mitigation

Only `firewall_monitor.rs` was changed:

- the real journal read now doubles as the availability check, removing one
  process launch per pass;
- the synchronous observation pass runs inside Tokio's blocking section rather
  than pinning the scheduler worker;
- empty or failed passes back off from the normal five-second cadence to a
  bounded 60-second ceiling, while a newly accepted denial resets the cadence
  to five seconds;
- seven-day retention maintenance runs at most hourly instead of rereading and
  rewriting the complete JSONL file every tick.

The active five-second cadence is retained after actual firewall activity, so
the mitigation bounds quiet/unavailable-host overhead without weakening the
normal observation response window.

## Verification

- BigBoy `172.20.0.130`, slot `firewall-cpu-test-r1`: the feature-gated
  `mackesd` firewall-monitor test filter passed `20/20`, including the new
  bounded-backoff regression.
- Target-only rustfmt passed on the synced BigBoy workspace.
- A crate-wide `cargo fmt -p mackesd -- --check` attempt on `.90`, slot
  `firewall-cpu-fmt-r1`, was not a useful source verdict because the dirty tree
  contains unrelated pre-existing formatting diffs across `mackesd`; the
  target-only check passed.
- No live seat was mutated, and no post-install CPU improvement is claimed
  here.

## Scope and blockers

No worklist, consolidated evidence, `boot_readiness.rs`, or unrelated file was
edited. Live attribution and post-install CPU sampling remain open because
this audit was source-only and the installed seat payload may differ from the
current dirty tree.
