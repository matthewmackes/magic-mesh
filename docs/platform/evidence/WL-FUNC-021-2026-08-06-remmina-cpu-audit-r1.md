# WL-FUNC-021 — Remmina peer-probe CPU audit (2026-08-06)

## Finding

`RemminaSyncWorker` performed three synchronous `TcpStream::connect_timeout`
probes per peer on every 60-second tick. The probes were already isolated with
`tokio::task::spawn_blocking`, so they did not block the async executor, but a
slow pass could make Tokio's default missed-tick behavior replay the next
cadences immediately. Permanently unavailable peers were also reprobed at the
full cadence, creating avoidable common-seat network/syscall churn.

## Source mitigation

`crates/mesh/mackesd/src/workers/remmina_sync.rs` now:

- sets the interval to `MissedTickBehavior::Delay`, preventing a slow blocking
  pass from producing a burst of immediate follow-up passes;
- retains probe results per peer and skips network I/O until that peer's bounded
  retry deadline;
- retries an all-closed peer at 60/120/240/480/900 seconds, capped at 900
  seconds, while any open protocol resets the peer to the normal 60-second
  cadence;
- prunes cache entries for peers that leave the registry; and
- keeps the existing `spawn_blocking` boundary and serial worker execution.

This preserves eventual discovery of a peer that returns while avoiding a
fixed-cadence retry storm. A peer with a live protocol is still refreshed every
60 seconds, and the honest negative result remains available between retries.

## Verification

The focused unit coverage adds a regression for the failure ladder, its 900
second ceiling, and success reset.

Farm `.50`, slot `remmina-cpu-audit-r1`:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=remmina-cpu-audit-r1 \
./install-helpers/xcp-build.sh cargo test -p mackesd \
workers::remmina_sync::tests:: --features async-services --locked -- --nocapture
```

Result: **12 passed, 0 failed**, 4,383 filtered out. Compilation completed
successfully; unrelated pre-existing warnings were emitted elsewhere in the
dirty `mackesd` crate. The disposable farm slot was removed after completion.

Live post-install CPU proof remains open and is not claimed by this source
audit.
