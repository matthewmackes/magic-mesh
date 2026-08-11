# WL-FUNC-021 seat-audio object identity — 2026-08-11

- Scope: volume restore binds a PipeWire node ID to its object serial and client
  process identity, then revalidates the fresh graph.
- Hostile boundary: a recycled node ID cannot receive the prior stream's restored
  volume.
- Focused gate: `cargo test -p mde-musicd seat_audio::tests::recycled_node_id_cannot_restore_volume_into_a_replacement_stream -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 10.2 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 257 filtered out.
- Remaining boundary: live PipeWire stream replacement proof remains.
