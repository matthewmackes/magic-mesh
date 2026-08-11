# WL-CRIT-007 restart readiness barrier — 2026-08-11

- Scope: the universal `boot_readiness` worker now publishes a fail-safe
  `phase: "probing", ready: false` snapshot before its node-phased first probe,
  replacing any optimistic record retained from an earlier daemon generation.
  If that barrier cannot publish, the worker fails and its installed
  `RestartPolicy::OnFailure` retries instead of exposing stale readiness. Around
  the existing publication sleep, Linux boot elapsed time is compared with
  monotonic elapsed time to detect suspend, while a bounded monotonic overrun
  detects long active scheduling gaps. Either event publishes the same barrier,
  clears cached probe/service/ping observations, and resets backoffs so every
  source must be re-observed before readiness can recover. No timer or periodic
  task was added.
- Production path: grouped daemon startup → universal `boot_readiness` worker
  → Bus `state/boot-readiness` → HOME boot-status consumers.
- Focused farm gates:
  - `172.20.0.50`, slot `1`:
  `workers::boot_readiness::tests::startup_barrier_supersedes_persisted_ready_snapshot`:
  PASS, 1 passed, 0 failed, 4,785 filtered out.
  - BigBoy `172.20.0.130`, slot `1`:
    `workers::boot_readiness::tests::wake_or_scheduling_gap_invalidates_cached_readiness_before_reuse`:
    PASS, 1 passed, 0 failed, 4,808 filtered out. The colliding `.50` attempt is
    discarded and not claimed.
- Remaining epic boundary: live hardware suspend/resume and broader release
  acceptance proof.
