# WL-FUNC-016 / WL-ARCH-009 — clipboard Bus recovery (r20)

Date: 2026-08-09

Base commit: `981d2ddef7a3f248c83f74628f453e073e0019c0`

Production source: `crates/mesh/mackesd/src/workers/clipboard_bridge.rs`

Source SHA-256: `f19960779ac90bda30209f9b4b80d608feeb43f9fee8288f5131d0893c1f467e`

## Correction

`ClipboardBridgeWorker` no longer exits permanently when the Bus spool cannot
open during startup. An explicit root remains authoritative; otherwise the
shared mde-bus resolver is used, with `mde_bus::SYSTEM_BUS_ROOT` as the daemon
fallback when no user root resolves. Startup open/prime failures retry at the
configured cadence clamped to 10 ms–2 s, and shutdown interrupts every retry
wait.

Cursor priming now returns an explicit failure instead of conflating an
unavailable Bus with an empty action history. The worker enters its live loop
only after a successful prime, preserving the existing rule that restart
backlog is transient and must not be re-applied. Live open/list failures likewise
retain the prior cursor and pending effects; a queued signed action is consumed
once after recovery. The exact-body authorizer, durable single-use replay
ledger, pending-before-new provider ordering, and echo/write guards are
unchanged. `OsClipboardAccess` also uses the system fallback and reports an open
failure as an error rather than reporting an absent root as an empty clipboard.

## Focused farm proof

Host: machine 194 (`172.20.0.170`)

Slot: `clipboard-bridge-bus-r20`

Each command used:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=clipboard-bridge-bus-r20 \
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services \
--lib workers::clipboard_bridge::tests::<TEST> -- --exact --nocapture
```

Exact tests and results:

- `clipboard_bus_root_preserves_resolved_root_and_has_system_fallback` —
  `1 passed; 0 failed; 4,427 filtered out`.
- `startup_bus_failure_recovers_forward_without_restart_and_stops_promptly` —
  `1 passed; 0 failed; 4,427 filtered out`.
- `transient_bus_read_failure_retains_cursor_and_queued_action` —
  `1 passed; 0 failed; 4,427 filtered out`.
- `run_loop_primes_past_the_backlog_and_exits_on_shutdown` —
  `1 passed; 0 failed; 4,427 filtered out`.
- `replayed_clipboard_capability_has_only_one_write_effect` —
  `1 passed; 0 failed; 4,427 filtered out`.

The third and subsequent checks used the already-synced slot directly after
unrelated in-progress `collab.rs` changes made the full dirty tree fail to
compile on three `&str` dereferences. Only the disposable remote slot received
the compiler-suggested three-line substitution; the local unrelated file was
not edited by this correction. The clipboard source in the slot matched the
recorded SHA-256.

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/clipboard_bridge.rs
```

Result: passed on machine 194. Local scoped `git diff --check` also passed.
No broad suite, package build, installed-seat proof, or unrelated test was run.
