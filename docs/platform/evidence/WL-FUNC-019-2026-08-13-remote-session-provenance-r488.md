# WL-FUNC-019 — remote-session provenance and replay boundary (r488)

Date: 2026-08-13

## Executable slice

The Remote Terminal client previously trusted records returned from a
`state/pty/<id>` Bus log without checking the record's embedded session ID,
peer, or worker sequence. A cross-session/cross-peer record could therefore
paint bytes or fold lifecycle state into the wrong pane, and a replayed sequence
could duplicate output or regress a live pane to a stale terminal state.

`crates/desktop/mde-term-egui/src/remote.rs` now:

- fails closed before rendering or lifecycle folding when a record's session ID
  or peer differs from the pane's bound resource;
- tracks the highest accepted worker sequence and ignores duplicate/regressive
  records before they can repaint bytes or cause a side effect;
- seeds that sequence from the validated pre-reattach log tail; and
- resets the sequence boundary when a bounded reconnect creates a new session.

The test Bus now assigns realistic monotonic sequences by default while allowing
exact hostile records for provenance/replay fixtures.

## Farm verification

- Host `.130` (`172.20.0.130`), slot
  `func019-remote-boundary-test-r488`:
  `cargo test -p mde-term-egui 'remote::tests::poll_' -- --nocapture` passed
  3/3 (`poll_rejects_cross_session_or_peer_records_before_they_can_render`,
  `poll_deduplicates_worker_sequences_before_output_or_lifecycle_fold`, and the
  existing output-to-grid fold), with 398 filtered out.
- Host `.130`, slot `func019-remote-boundary-clippy-r488`:
  `cargo clippy -p mde-term-egui --all-targets -- -D warnings` passed.
- Host `.130`, synced workspace
  `magic-mesh-farm-func019-remote-boundary-test-r488`:
  `rustfmt --edition 2021 --check crates/desktop/mde-term-egui/src/remote.rs`
  passed.

An initial `.170` clippy route was rejected before synchronization because its
6.4 GiB free `/home` was below the farm helper's 8 GiB safety floor. The gate was
rerouted to `.130`; this was a capacity refusal, not a code failure.

## Acceptance advanced and remaining

This advances WL-FUNC-019 acceptance criteria 1–3 at the Remote Terminal
resource/session boundary: state is now identity/provenance-bound, replay cannot
fabricate output or lifecycle effects, and violations produce an observable
failure. Universal resource-kind coverage, the remaining typed action routes,
and deferred post-release live loss/rejoin/login proof remain owned by the
canonical epic.
