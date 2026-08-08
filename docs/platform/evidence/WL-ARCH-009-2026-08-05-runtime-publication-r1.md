# WL-ARCH-009 — bounded worker runtime publication (2026-08-05)

The admitted WorkerContract/WorkerRuntimeSnapshot projection now has a real
daemon publication seam. A caller supplies the observation, clock, and already
open `mde-bus` persistence handle; the seam writes the same bounded,
credential-free JSON to the canonical per-worker and node topics and returns
both retained message identities. Contract, freshness, secret-shaped content,
capacity, topic, and persistence failures fail closed. No process state or
fallback observation is inferred.

## Verification

- BigBoy `.130`, slot `wl-arch009-status-publish-r1`:
  `cargo test -p mackesd worker_runtime_status -- --nocapture`.
- Result: `7 passed; 0 failed; 4420 filtered out`.
- Actual process sampling, split-service supervision, `/run/mde/mackesd-status.json`,
  and live fleet publication remain open.
