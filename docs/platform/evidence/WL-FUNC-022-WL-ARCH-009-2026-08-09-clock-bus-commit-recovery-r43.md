# WL-FUNC-022 / WL-ARCH-009 — Clock Bus and commit recovery (r43)

Date: 2026-08-09

## Scope

- Production source: `crates/mesh/mackesd/src/workers/clock.rs`
- Baseline commit: `fdc6187d4a857c907aecdead54dc01b99b955b23`
- No worklist edit, commit, or push.

## Corrected semantics

- `ClockWorker::new` no longer freezes an absent default Bus root. Activation resolves an explicit/test root first, then the current user/default root, and finally canonical `mde_bus::SYSTEM_BUS_ROOT`.
- Durable Clock authority loads independently of Bus availability. An unopenable startup Bus is retried by the same worker with shutdown-aware exponential backoff bounded from 10 ms to 2 s.
- Runtime keeps one `Persist` handle and calls `reopen_if_index_changed()` before staging reads, so atomically replaced indexes and external forward writes remain visible.
- The durable command lane is not tail-skipped: its persisted action cursor continues to fold outage history. Command and audio-status reads are completed before deadline/action effects; a Bus read error defers the complete effect sweep.
- Deadline and command mutation use in-memory checkpoints. Failed durable commit or required Clock-state publication restores the prior snapshot, action cursor, publication flag, and send/cursor maps. A same-process retry therefore reprocesses the action.
- If commit succeeded but publication failed, replay reloads the durable winner and republishes it before treating the boundary as successful. Audio status cursors likewise do not advance across failed durable acknowledgement.

## Focused hostile regressions

All successful verification ran in slot `clock-bus-r43` on machine194, which identified itself as `mcnf-build-xen-194` at its documented live address `172.20.0.170`.

1. `workers::clock::tests::clock_bus_root_honors_override_and_falls_back_to_system_spool`
   - Result: PASS (`1 passed; 0 failed; 4485 filtered out`).
2. `workers::clock::tests::late_bus_recovers_same_worker_and_observes_external_forward_command`
   - Result: PASS (`1 passed; 0 failed; 4485 filtered out`).
   - Proves durable state loads while Bus open is blocked, shutdown-aware same-worker recovery, and a command written after activation through a separate `Persist` handle executes.
3. `workers::clock::tests::commit_and_publication_failures_retain_action_for_same_worker_retry`
   - Result: PASS (`1 passed; 0 failed; 4485 filtered out`).
   - Proves both an injected commit failure and a commit-success/state-publication-failure leave the action retryable by the same live worker; the latter reloads and republishes revision 3.
4. `workers::clock::tests::audio_acknowledgement_failure_retains_status_for_same_worker_retry`
   - Result: PASS (`1 passed; 0 failed; 4485 filtered out`).
   - Proves an injected durable acknowledgement error retains the previously completed audio-status cursor; the same worker reads the row again, calls acknowledge a second time, and advances only after the call returns successfully with `changed = false`.

Exact final-source commands:

```text
ssh -i /root/.ssh/mackes_mesh_ed25519 -o BatchMode=yes mm@172.20.0.170 "source \$HOME/.cargo/env 2>/dev/null; cd magic-mesh-farm-clock-bus-r43 && rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/clock.rs && cargo test -p mackesd --lib --features async-services workers::clock::tests::audio_acknowledgement_failure_retains_status_for_same_worker_retry -- --exact --nocapture"
ssh -i /root/.ssh/mackes_mesh_ed25519 -o BatchMode=yes mm@172.20.0.170 "cd magic-mesh-farm-clock-bus-r43 && target/debug/deps/mackesd_core-7b2dac935c32c5ff workers::clock::tests::clock_bus_root_honors_override_and_falls_back_to_system_spool --exact --nocapture"
ssh -i /root/.ssh/mackes_mesh_ed25519 -o BatchMode=yes mm@172.20.0.170 "cd magic-mesh-farm-clock-bus-r43 && target/debug/deps/mackesd_core-7b2dac935c32c5ff workers::clock::tests::late_bus_recovers_same_worker_and_observes_external_forward_command --exact --nocapture"
ssh -i /root/.ssh/mackes_mesh_ed25519 -o BatchMode=yes mm@172.20.0.170 "cd magic-mesh-farm-clock-bus-r43 && target/debug/deps/mackesd_core-7b2dac935c32c5ff workers::clock::tests::commit_and_publication_failures_retain_action_for_same_worker_retry --exact --nocapture"
ssh -i /root/.ssh/mackes_mesh_ed25519 -o BatchMode=yes mm@172.20.0.170 "cd magic-mesh-farm-clock-bus-r43 && rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/clock.rs"
git show HEAD:crates/mesh/mackesd/src/workers/clock.rs | ssh -i /root/.ssh/mackes_mesh_ed25519 -o BatchMode=yes mm@172.20.0.170 'baseline=$(mktemp); tee "$baseline" >/dev/null; cd magic-mesh-farm-clock-bus-r43; git diff --no-index --check -- "$baseline" crates/mesh/mackesd/src/workers/clock.rs; result=$?; rm -f "$baseline"; exit $result'
```

The scoped rustfmt and diff checks passed. A workspace-wide `cargo fmt --all -- --check` was also attempted and reported pre-existing formatting differences in unrelated shared-worktree files; none were changed under this slice.

## Hashes and endpoint discrepancy

- Final `clock.rs` SHA-256, local and farm: `ef9a9cc3798ee809e93c268ab07f8cc66204d0597da262db370e45f267b3b546`
- Scoped `clock.rs` patch SHA-256: `0f8dc32e042f91c9273c8b47832ae649811558e8b161215750f5eaff53315665`

The requested literal endpoint `192.168.23.170` timed out on SSH. The farm roster maps machine194 to `172.20.0.170`, and a direct identity check there returned hostname `mcnf-build-xen-194` with only `172.20.0.170/16`. Verification therefore completed on the requested machine and slot through its live documented address; literal-address verification remains impossible until routing or addressing for `192.168.23.170` is corrected.
