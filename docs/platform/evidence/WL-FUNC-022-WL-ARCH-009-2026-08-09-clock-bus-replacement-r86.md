# WL-FUNC-022 / WL-ARCH-009 — Clock Bus replacement recovery r86

Date: 2026-08-09

## Scope

- Production and regression source: `crates/mesh/mackesd/src/workers/clock.rs`
- Baseline commit inspected: `0e13c75e9077f1f0d0151032d8f9651508a81fce`
- Verification host: machine194 build VM `mcnf-build-xen-194`, `172.20.0.170`
- Isolated farm slot: `clock-bus-r86`
- No worklist edit, commit, or push was made by this slice.

## Correctness model

- Every sweep fresh-resolves the configured/default Bus root, observes the live
  `index.sqlite` device/inode before opening, opens a new `Persist`, and accepts
  the connection only when the path identity is unchanged after open. An absent
  late Bus is initialized with a discarded connection and then bracketed again.
- The opened connection, root, and identity form one `ClockBusTransaction`.
  Commands, audio statuses, and approved-peer state are all read from that one
  connection and the path identity is rechecked after the complete source stage,
  before and after durable Clock commits/audio acknowledgements, and after every
  Bus publication. A generation change fails the sweep instead of combining a
  retired read or write with current effects.
- Bus activation stages both transient input-lane tails and peer state before it
  mutates worker state. On same-path replacement, the retained command and audio
  tails become the new generation floors together; retry timestamps are cleared,
  the durable Clock authority/action cursor is committed, retained state is
  republished, and durable pending audio plus peer convergence are replayed. The
  active identity is installed only after all required work and a final identity
  check succeed. A failed activation restores the in-memory checkpoint.
- The SQLite Clock authority, request ledger, action cursor, and audio outbox are
  not reset during Bus activation. They remain the durable exactly-once/replay
  boundary. If a Bus swap follows a successful durable commit, the failed
  transaction is repaired by loading/republishing the durable winner during the
  next generation activation. If a swap follows an audio acknowledgement, its
  durable acknowledgement remains authoritative while the retained replacement
  status is skipped.
- Writes update `published_once`, audio retry timestamps, or peer retry timestamps
  only after the opened connection wrote successfully and the live path still
  names the same index. The next fresh-open sweep therefore detects even a swap
  immediately after a final check and activates the replacement without a daemon
  restart.

## Focused hostile verification

The final source compiled as part of the exact test command. Results:

1. `workers::clock::tests::same_path_bus_replacement_skips_retained_lanes_and_consumes_forward_once`
   - PASS: `1 passed; 0 failed; 4617 filtered out`.
   - A running worker first commits a schedule, then its Bus index is atomically
     replaced at the same path by an index retaining a valid command and valid
     audio status. Activation preserves revision 2 and the committed schedule,
     skips both retained rows, binds the replacement identity, consumes the first
     forward command and audio status once, reaches revision 3, and remains at
     revision 3 with one forward schedule and one acknowledgement call on the
     next sweep. The durable authority cursor equals the forward command ULID.
2. `workers::clock::tests::commit_and_publication_failures_retain_action_for_same_worker_retry`
   - PASS: `1 passed; 0 failed; 4617 filtered out`.
   - Retains the r43 durable-commit and commit-success/publication-failure replay
     boundary under the transaction API.
3. `workers::clock::tests::audio_acknowledgement_failure_retains_status_for_same_worker_retry`
   - PASS: `1 passed; 0 failed; 4617 filtered out`.
   - Retains the durable audio acknowledgement/cursor retry boundary.
4. `workers::clock::tests::late_bus_recovers_same_worker_and_observes_external_forward_command`
   - PASS: `1 passed; 0 failed; 4617 filtered out`.
   - Retains late-Bus same-worker startup recovery and forward-command handling.

Primary routed commands:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=clock-bus-r86 install-helpers/xcp-build.sh cargo test -p mackesd --lib --features async-services workers::clock::tests::same_path_bus_replacement_skips_retained_lanes_and_consumes_forward_once -- --exact --nocapture
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=clock-bus-r86 install-helpers/xcp-build.sh cargo test -p mackesd --lib --features async-services workers::clock::tests::commit_and_publication_failures_retain_action_for_same_worker_retry -- --exact --nocapture
```

After the explicit routed sync, the two already-built exact r43 regressions and
scoped formatter were run directly in
`/home/mm/magic-mesh-farm-clock-bus-r86`. The final formatted source was then
recompiled and the new replacement regression rerun; it passed in 55.57 seconds.
`rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/clock.rs` and
`git diff --check -- crates/mesh/mackesd/src/workers/clock.rs` passed.

## Hashes and limits

- Final `clock.rs` SHA-256, identical locally and on machine194:
  `05f64ec271c4189866c86349949da5bdad6fcc511c76aa4a5bf92f36df25ee0d`
- Owned `clock.rs` patch SHA-256:
  `adf4f70fb0117c3079b048e269e31f6389ff5bda7180269442426c686a278db8`

The first cold farm compile encountered a concurrent incomplete
`node_availability.rs` edit; after its owner completed that shared edit, the
same r86 slot compiled the shared crate and all Clock gates above passed. There
is no remaining source or farm blocker for this slice.

This is persisted-Bus and durable SQLite recovery evidence. It does not claim a
live ringing alarm, mde-musicd playback, PipeWire route, speaker, or other audio
hardware proof.
