# WL-CRIT-007 / WL-UX-013 — health generation restart recovery (r17)

Date: 2026-08-09

## Live finding

After Dell peer publication recovered, a read-only follow-up proved Dell and
seat 15 mutually online with fresh rows and active observation/shell services.
The same Dell journal exposed a separate restart fault:

```text
health-reconciler: canonical health file rejected; retaining last valid projection
node-health generation did not advance: retained 4527, candidate 136
```

The producer's generation was process-local and restarted at zero. Durable
ingress correctly retained its anti-replay high-water, so it rejected every new
publication until the ten-second producer counter would eventually exceed
4,527. Peer presence was live, but health history could not advance.

## Correction

On its first cycle after process start, `NodeGradeWorker` now reads the bounded,
non-symlink canonical row and derives a restart floor from the larger of its
generation and publication timestamp, capped at the current clock. It emits the
next generation above that floor. Later cycles retain the ordinary in-memory
increment, so action generation checks and same-process behavior do not change.

Using publication time as a repair floor handles both clean restart and the
already-observed failure: even if the broken producer overwrote generation
4,527 with `136`, that row still carries a fresh Unix-ms publication timestamp
and the corrected producer immediately advances beyond the durable ingress
cursor. No checkpoint deletion, replay-ledger weakening, or fabricated
acceptance is used.

## Focused verification

Machine 193 (`172.20.0.90`), slot `crit007-etcd-failover-r1`:

```text
cargo test -p mackesd --lib --features async-services \
  restart_generation_uses_durable_publication_floor_after_counter_rollback \
  -- --nocapture
```

Result: PASS — 1 passed, 0 failed, 4,387 filtered out. `git diff --check`
passed. A whole-file `rustfmt --check` remains red on an unrelated pre-existing
assertion block in the same module; the changed hunks require no formatting
rewrite and no broad test was run.

The corrected source is committed for the next governed candidate. It was not
hot-copied onto Dell; therefore this record does not claim Dell's installed
health producer has advanced yet.
