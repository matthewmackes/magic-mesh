# WL-ARCH-009 uncensused runtime refusal — 2026-08-09

## Outcome

The unified Workers runtime sampler now refuses a complete node projection when
the supervisor reports any worker absent from the canonical registry. It no
longer silently drops an active, unowned process and publishes an apparently
complete snapshot. After the drift is corrected, sampling resumes without
advancing lifecycle generation during the rejected attempt.

## Farm verification

- BigBoy (`172.20.0.130`), slot `arch009-r4-20260809`:
  - exact hostile regression passed 1/1;
  - complete `workers::worker_runtime_status::tests` slice passed 15/15;
  - exact-file `rustfmt --check` passed.
- Tests ran with `--offline` because concurrent workspace manifest changes made
  the synced lockfile stale under `--locked`; no repository lockfile was edited.

## Source hash

- `dd5ae599a4a722eb9cd267e40cd25e74f4f437c782ee8cbb2a4ade2d3fa8fbe9` —
  `crates/mesh/mackesd/src/workers/worker_runtime_status.rs`

This closes one registry/runtime-truth gap; ARCH-009 remains open for its wider
provider, UI, live-census, and fleet-isolation acceptance work.
