# Compute exposure Bus transaction recovery r89

Date: 2026-08-09

Scope: `WL-ARCH-010`, `WL-ARCH-009`

## Correctness model

- Every active sweep resolves the Bus root again, fresh-opens `Persist`, and binds the connection to the `index.sqlite` device/inode observed at the same path. A path/connection mismatch is deferred without mutation.
- The expose and unexpose lanes are read completely into one staged generation before any authorization claim or firewalld effect. Each lane fails closed above 4,096 messages or 16 MiB of bodies; there is no truncated-prefix execution.
- Every capability claim, firewall transaction, terminal result publication, and exposed-state publication is guarded by the staged generation identity. A same-path replacement aborts the old sweep and is activated on the next fresh acquisition.
- Replacement activation atomically floors both command lanes at the replacement's current tails. Retained replacement rows cannot repeat an external effect; the first corrected-forward row is processed once.
- The pre-existing root-owned atomic action journal remains the write-ahead authority. It records the exact planned rules before consuming the capability, terminalizes observed applied/partial/failed state, and republishes a missing exact result without rerunning firewalld. Restart recovery rebuilds active rules from firewalld before resolving prepared journal entries.

## Focused farm proof

Host: farm machine196 (`172.20.0.196`)

Slot: `/home/mm/magic-mesh-farm-compute-expose-r89`

The slot was created from `git archive HEAD` and overlaid only with `compute_expose.rs`; concurrent worktree changes were not copied.

Commands and results:

```text
cargo test -p mackesd --features async-services --lib \
  workers::compute_expose::tests::same_path_bus_replacement_skips_retained_request_and_runs_forward_once \
  -- --exact --nocapture
PASS: 1 passed; 0 failed; 4605 filtered out

target/debug/deps/mackesd_core-7b2dac935c32c5ff --exact \
  workers::compute_expose::tests::transient_bus_resolution_and_open_failure_recovers_forward_without_restart \
  --nocapture
PASS: 1 passed; 0 failed; 4605 filtered out

target/debug/deps/mackesd_core-7b2dac935c32c5ff --exact \
  workers::compute_expose::tests::failed_expose_reply_retry_and_reload_counts_are_honest \
  --nocapture
PASS: 1 passed; 0 failed; 4605 filtered out

rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/compute_expose.rs
PASS
```

The first final-source relink found machine196 at 99% `/home` usage and failed with `mold: Disk full`. Three completed disposable farm slots were removed, restoring 14 GiB free, and the exact final-source compile/test then passed. This was farm-capacity recovery, not a source correction.

## Hashes

- `compute_expose.rs`: `40626426a04c206f65c38610f50b80ade6ee7669498bfda93bfbf7ea18f96133`
- owned source patch: `75b4d6f6467119440680d7abb3957d91020350817b505f9049ee8e6861a2834c`

## Claim boundary

This proves deterministic worker recovery with a real SQLite Bus and injected firewalld/result boundaries. It does not claim a live host firewall mutation or installed-seat proof.
