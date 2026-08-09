# WL-FUNC-017 / WL-ARCH-009 — Caltrans overlay Bus recovery (r54)

Date: 2026-08-09

Scope was limited to `crates/mesh/mackesd/src/workers/caltrans_camera_overlay.rs` and this evidence file. `docs/platform/WORKLIST.md` was not edited. No commit or push was made.

## Production semantics

- Construction no longer freezes `default_bus_root()` as an optional value. Each read or publication transaction resolves an explicit override first, then the current user/service Bus root, then canonical `mde_bus::SYSTEM_BUS_ROOT`.
- Every transaction fresh-opens Persist and follows a replaced index before reading or writing. A late, temporarily unopenable, or atomically replaced Bus therefore recovers in the same worker.
- Vehicle context is now `Result<Option<CameraContext>>`. Bus open/read errors, missing message bodies, JSON decode failures, wrong-host rows, and structurally invalid fixes return an error and defer the complete effect. They cannot masquerade as missing context, clear `last_good`, or publish a false empty camera projection.
- A successfully read offline/no-fix, stale, or valid point outside Caltrans coverage returns `Ok(None)`. Only that path may publish the honest empty snapshot, and `last_good` is cleared only after that publication durably succeeds.
- Provider I/O remains outside the Bus transaction. Before admitting its result, the worker fresh-opens the Bus and re-reads vehicle context; a changed or lost context prevents stale camera admission.
- Every camera publication returns `io::Result`. Fresh, paused, and empty snapshots mutate `last_good` and report refresh success only after the Persist write succeeds. Failed serialization/writes retain prior in-memory state, remain retryable, and continue incrementing the Bus publish-error metric.
- Shutdown remains selectable during retry sleeps and in-flight provider work.

## Focused verification

Farm host: machine194, `172.20.0.170`

Explicit slot: `MCNF_BUILD_HOST=172.20.0.170`, `MCNF_BUILD_SLOT=caltrans-overlay-bus-r54`

The slot was created from `git archive HEAD`; only the owned worker source was overlaid. The final affected test build used:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=caltrans-overlay-bus-r54 \
ssh mm@172.20.0.170 'cd magic-mesh-farm-caltrans-overlay-bus-r54 && \
  cargo test -p mackesd --lib \
  workers::caltrans_camera_overlay::tests::vehicle_read_or_decode_failure_is_effect_free \
  -- --exact --nocapture'
```

Result: pass. The clean slot emitted 256 existing crate warnings and no errors.

The final built test binary was then run with `--exact` for:

```text
workers::caltrans_camera_overlay::tests::vehicle_context_requires_fresh_same_host_california_fix
workers::caltrans_camera_overlay::tests::vehicle_read_or_decode_failure_is_effect_free
workers::caltrans_camera_overlay::tests::failed_publication_retains_state_and_corrects_forward
workers::caltrans_camera_overlay::tests::late_and_replaced_bus_recovers_in_the_same_worker
```

Results: four tests passed individually; each reported `1 passed; 0 failed; 4510 filtered out`. Durations were 0.00 s, 0.00 s, 0.00 s, and 0.09 s.

Operational coverage:

- The late/replaced test starts with an unopenable Bus root, activates it without restarting the worker, publishes a fresh vehicle fix, replaces `index.sqlite`, publishes a changed fix through the replacement, and observes the corrected camera query point from the same worker. It also asserts canonical system fallback and prompt shutdown.
- The read/decode test injects both a lane read error and malformed vehicle JSON and proves no publication or `last_good` mutation.
- The write-failure test proves failed fresh publication retains the prior snapshot, the same prepared state corrects forward after the writer recovers, and failed empty publication cannot clear `last_good`; the corrected empty publication then clears it.

Scoped farm formatting:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=caltrans-overlay-bus-r54 \
ssh mm@172.20.0.170 'cd magic-mesh-farm-caltrans-overlay-bus-r54 && \
  rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/caltrans_camera_overlay.rs'
```

Result: pass.

The scoped farm diff check compared the committed source with the slot file using `git diff --no-index --check`.

Result: pass.

## Hashes

```text
f2df20d40759a33d5e12e1e30fe15505e39e35de50544d3d3e1d16f48196f1e8  crates/mesh/mackesd/src/workers/caltrans_camera_overlay.rs
4d0c33ce45ee917e4da8bdb89f47c15f1548c53dbb6a20627a8bdb5140ca03f4  scoped source patch against HEAD
```

The local and machine194 source hashes matched exactly.

## Blockers

None.
