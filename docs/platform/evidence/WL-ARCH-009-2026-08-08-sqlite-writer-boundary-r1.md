# WL-ARCH-009 SQLite writer boundary — 2026-08-08

This checkpoint establishes a real but deliberately partial persistent-writer
boundary. The control process binds `/run/mackesd/store-writer.sock`, migrates
the database before returning startup success, and owns one long-lived writable
connection on a dedicated thread. The protocol is schema-versioned, accepts no
SQL text, caps each frame at 64 KiB, uses a root-only `0700` directory and `0600`
socket, rejects a second live owner, removes a stale socket, and drains on daemon
shutdown.

Seven canonical store mutations now cross that boundary in every split `serve`
process: audit insertion, rollback revision, node role, node health, node
version, credential refresh, and node upsert. Clients reconnect per request,
wait at most two seconds for owner recovery, and return an explicit error when
the owner remains absent. Restart tests prove state survives owner replacement.

The five non-control units declare both `Requires=mackesd-control.service` and
`After=mackesd-control.service`. Because control starts and migrates the writer
before its existing `READY=1`, first boot is ordered behind schema/socket
readiness. The process-boundary validator and its hostile fixture now enforce
that ordering. Stale compile-time tests that still included the deleted
`mackesd.service` and stop-policy drop-in were cut over to the six grouped units
and the compute credential owner.

## Negative authority inventory and honest residual

Global read-only enforcement is intentionally **not** enabled. A first attempt
would have rejected still-live direct writes silently. The checked baseline
records 61 conservative direct `execute`/`execute_batch`/transaction syntax
sites across 15 non-store source files (including embedded test fixtures).
`lint-mackesd-sqlite-authority.sh` fails if that set grows and rejects any
SQL-shaped writer operation. Those sites must be classified and migrated before
the one-writer acceptance criterion can be claimed; ordinary connections retain
their prior behavior meanwhile.

## Focused verification

Fedora farm `.90`, explicit slot `arch009-sqlite-writer-20260808-r3`:

```text
cargo test --target-dir /home/mm/magic-mesh-farm/target \
  -p mackesd store::writer::tests -- --nocapture
4 passed; 0 failed; 4369 filtered out
```

The hostile suite covers unknown schema, oversized frames, raw writes through
an explicitly read-only connection, bounded missing-owner readiness, owner
restart, and post-hostile-request availability. The package compiled all
mackesd test targets; existing repository warnings remained warnings.

Local source checks:

```text
lint-mackesd-sqlite-authority.sh --self-test: PASS
lint-mackesd-sqlite-authority.sh: PASS (61 reviewed residual syntax sites)
verify-mackesd-process-boundary.py --self-test: PASS
verify-mackesd-process-boundary.py: PASS
git diff --check (scoped): PASS
```

Scoped source/contract digest: `6113f2114424fd872c24aa84869cd0010bf13206d402d93b6128c2acdb22f2ba`.

## Remaining acceptance gap

This is not proof of one SQLite writer across all six groups. The residual
direct-write inventory still needs operation-by-operation ownership migration,
then global non-owner read-only enforcement and a built-RPM six-process
crash/recovery drill. No live hardware/runtime claim is made here.
