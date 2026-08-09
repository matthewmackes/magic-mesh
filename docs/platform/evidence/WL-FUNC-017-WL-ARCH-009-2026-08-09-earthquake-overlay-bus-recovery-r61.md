# WL-FUNC-017 / WL-ARCH-009 — Earthquake Overlay Bus recovery (r61)

Date: 2026-08-09

Farm: BigBoy `172.20.0.130`, slot `earthquake-overlay-bus-r61`

## Production semantics

- The worker no longer freezes `default_bus_root()` during construction or uses best-effort publication. Every projection transaction resolves an explicit override, then the current user/environment root, then canonical `mde_bus::SYSTEM_BUS_ROOT`; it opens a fresh `Persist` and requires the state-topic write to succeed.
- A late Bus therefore requires no worker restart. Fresh-open publication also follows an atomically replaced `index.sqlite` on the next transaction while preserving the retained latest-wins `state/overlay/usgs-earthquakes/<node>` contract.
- Modified snapshots, conditional-304 refresh timestamps, and degraded last-good gaps are cloned before publication. `last_good` changes only after the required Bus row succeeds; open/serialize/write failure is effect-free for that private authority.
- Poll results distinguish fresh success, successfully published degradation, and deferred publication. Deferred publication sleeps at the current bounded retry delay but does not increase/reset retry cadence; successful fresh publication resets retry and normal polling resumes.
- Conditional HTTP validators are cleared whenever a fresh response transaction does not commit. The next request asks for a complete body, preventing an ETag/Last-Modified 304 from stranding a response that never reached the Bus.
- The Earthquake worker has no separate retained Bus context/read lane. Feed fetch, bounded-body, and decode errors cannot masquerade as an empty successful snapshot: with last-good state, an honest gap is committed only after publication; without last-good state, no projection/private snapshot effect occurs.

## Focused hostile coverage

- `same_worker_recovers_late_and_replaced_bus_with_latest_wins_state`: begins with an unopenable Bus path and verifies no last-good commit, publishes after that path becomes available, atomically replaces `index.sqlite`, and verifies the same worker's next 304 transaction publishes into the replacement index. It also checks canonical system fallback.
- `failed_publication_corrects_forward_without_state_or_cadence_advance`: injects failures for an initial modified publication and a later 304 refresh, verifies last-good/timestamp and retry cadence remain unchanged, then verifies both transactions correct forward and only successful rows are retained.

## Verification

The farm helper explicitly selected BigBoy and the requested slot:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=earthquake-overlay-bus-r61 \
install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  --features async-services \
  workers::earthquake_overlay::tests::same_worker_recovers_late_and_replaced_bus_with_latest_wins_state \
  -- --exact --nocapture

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4535 filtered out
```

Final-source warmed-slot checks:

```text
rustfmt --edition 2021 --config skip_children=true --check \
  crates/mesh/mackesd/src/workers/earthquake_overlay.rs
# exit 0 on BigBoy

cargo test -p mackesd --lib --features async-services \
  workers::earthquake_overlay::tests::same_worker_recovers_late_and_replaced_bus_with_latest_wins_state \
  -- --exact --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4535 filtered out

cargo test -p mackesd --lib --features async-services \
  workers::earthquake_overlay::tests::failed_publication_corrects_forward_without_state_or_cadence_advance \
  -- --exact --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4535 filtered out

git diff --check -- crates/mesh/mackesd/src/workers/earthquake_overlay.rs
# exit 0
```

## Residual caveats

- The HTTP response and Bus row are not one atomic operation. On a failed Bus publication, the worker deliberately discards conditional validators and refetches a complete feed; this may consume extra bandwidth but prevents false 304 convergence and private-state advancement.
- The first farm compile was blocked outside this ownership by a concurrent `firms_overlay.rs` test expecting a tuple after that worker changed to `ApplyOutcome`. The disposable r61 remote slot overlaid only the unrelated FIRMS file from `HEAD` for isolated Earthquake verification; no local concurrent file was edited or reverted.
- Existing crate-wide warnings were emitted by unrelated modules; neither focused test produced a scoped warning or failure.

## Hash

```text
3d6b8fa115812dd7d3fdf24522de5a803df3e1a24f7007890c1594bc0bc69daf  crates/mesh/mackesd/src/workers/earthquake_overlay.rs
```

No WORKLIST edit, commit, or push was performed.
