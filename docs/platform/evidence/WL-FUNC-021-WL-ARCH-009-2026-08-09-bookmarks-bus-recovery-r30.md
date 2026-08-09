# Bookmarks late-Bus recovery checkpoint (R30)

Date: 2026-08-09

Worklist: `WL-FUNC-021`, `WL-ARCH-009`
Base commit: `f1ab84b229866cba77a5cc69b53f4be0b3e961ae`

## Runtime semantics

The `bookmarks` worker no longer exits successfully and permanently when its
Bus root is unresolved or temporarily unopenable. It resolves an explicit or
user Bus first and falls back to `mde_bus::SYSTEM_BUS_ROOT`, then retries Bus
open and request-topic activation in the same worker with shutdown-aware
exponential backoff bounded from 10 ms to 2 s.

Every `action/bookmarks/<verb>` topic is a transient command lane, including
the observational `check-links` command. Existing startup topics are discovered
and tail-primed as one activation transaction; one failed tail read installs no
partial cursor map. Topics created after activation are forward work and remain
absent from that map, so `list_since(None)` executes their first message and the
installed message cursor executes each later message once.

Each steady-state command sweep also reads every discovered topic into a
candidate batch before moving any cursor, consuming an authorization nonce, or
applying an effect. A `list_topics` or `list_since` failure aborts the complete
sweep rather than treating the failed lane as empty or partially converging
effects from the remaining lanes.

Bookmark state/history is not a transient Bus lane: the durable CRDT snapshot
and append-only segment live in the node-local and Syncthing stores. The worker
loads and folds those files before waiting for Bus activation, then publishes
the honest converged collection after activation. The recovery proof seeds a
durable local op during the Bus outage and observes it in the first published
collection.

## Focused farm verification

Host: machine9 (`172.20.0.50`)
Slot: `bookmarks-bus-r30`

The source was synced to that explicit farm slot and these exact affected tests
were run:

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::bookmarks::tests::service_bus_root_falls_back_to_the_shared_system_spool \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::bookmarks::tests::bus_read_failure_leaves_the_complete_command_sweep_untouched \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::bookmarks::tests::late_bus_folds_durable_history_and_processes_new_topic_from_first_message \
  -- --exact --nocapture
```

Each command passed: `1 passed; 0 failed; 4,450 filtered out`. The recovery
test proves one worker survives an unresolved root result, an open error, and an
atomic activation-tail failure; suppresses the retained startup command; folds
durable outage history; executes the first and second signed actions on a topic
created after activation; and exits promptly on shutdown. The read-failure test
proves two ready command lanes cause zero cursor movement, nonce consumption,
or bookmark ops when either lane read fails, then both apply once after recovery.

Exact formatting and scoped diff checks passed:

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/bookmarks.rs
git diff --check -- crates/mesh/mackesd/src/workers/bookmarks.rs
```

The first helper-driven farm compile was contaminated by an unrelated,
concurrent uncommitted `adfilter.rs` slice and failed on that file before
reaching bookmarks. No local `adfilter.rs` change was touched. The ephemeral
machine9 slot copy of only `adfilter.rs` was restored to `HEAD`, after which all
three bookmarks tests above compiled and passed. This is not a bookmarks
blocker.

Source SHA-256:
`2b34ddf383474b5dd9a503f17b4a9a3eb73425c338744172fe42806da7e364dc`.

## Scope

No broad suite, package build, live seat proof, WORKLIST edit, or unrelated test
was run. This checkpoint is limited to bookmarks Bus-root selection, startup
recovery and activation, transient command replay boundaries, Bus read-failure
atomicity, and durable bookmark-history folding.
