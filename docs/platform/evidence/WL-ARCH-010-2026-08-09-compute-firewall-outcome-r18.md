# WL-ARCH-010 compute firewall outcome boundary r18 — 2026-08-09

The compute exposure worker now verifies the exact signed body before writing a
root-worker-local action reservation, consuming the one-use capability, or
touching firewalld. The bounded journal lives at
`/var/lib/mackesd/compute-expose/action-journal.json`; it is mode-0600,
owner-checked, size/schema/invariant checked, and replaced with the existing
fsync-plus-rename atomic sealed-file helper. It contains typed action/rule facts
and bounded results, never the armed token, raw command, or stderr.

An authorized action is cursor-acknowledged only after its ULID-correlated
`reply/<request-ulid>` result is published. A crash after capability consumption
recovers a terminal record directly, or reconciles a prepared record against the
startup-seeded active firewalld shadow without reauthorizing or rerunning the
mutation. Applied, partial, and failed counts mean active projection changes:
reload failure is partial with `applied=0` and every attempted change counted as
failed-to-activate. Failed additions and removals do not fabricate active-state
changes. Mesh unexpose now uses the same local Nebula destination address as
expose, so its exact rich-rule removal body matches the prior addition.

## Focused farm verification

Machine 194 (`172.20.0.170`), slot `arch010-firewall-outcome-r18`, clean tracked
HEAD plus only `compute_expose.rs` because unrelated shared-worktree
`node_grade.rs` edits did not compile:

```text
cargo test -p mackesd \
  workers::compute_expose::tests::failed_expose_reply_retry_and_reload_counts_are_honest \
  -- --exact
```

Result: **1 passed, 0 failed, 4,398 filtered out**. The regression recreates the
worker after an exact-reply failure, proves durable terminal replay, simulates a
crash with only the prepared reservation after nonce consumption, proves no
mutation rerun, checks failed and reload-partial counts, rejects fabricated
active rules, and compares the Mesh add/remove rich-rule bodies exactly.

Source SHA-256:
`5146c06162733a5e2d229de456033dd6ad96494e25a747e7b26889655e469e2e`.

## Remaining limitations

This focused checkpoint does not claim package or installed-seat firewalld
proof. Prepared-action restart classification depends on the worker startup
seed having read the real active firewalld rules before polling, as the
production run path does. The journal retains at most 1,024 action records;
older exact replies remain in Bus persistence but cannot be reconstructed from
the local journal after that bounded retention window.
