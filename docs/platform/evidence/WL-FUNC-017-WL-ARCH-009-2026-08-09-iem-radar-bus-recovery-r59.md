# WL-FUNC-017 / WL-ARCH-009 — IEM radar Bus recovery (r59)

Date: 2026-08-09

Scope was limited to `crates/mesh/mackesd/src/workers/iem_radar_overlay.rs` and this evidence file. `docs/platform/WORKLIST.md` was not edited. No commit or push was made.

## Production semantics

- Construction no longer freezes an optional Bus root. Every read and publication transaction resolves an explicit override first, then the current user/service Bus root, then canonical `mde_bus::SYSTEM_BUS_ROOT`.
- Every transaction fresh-opens Persist and follows a replaced index before reading or writing. A late, temporarily unopenable, or atomically replaced Bus therefore recovers in the same worker with bounded shutdown-aware retry.
- Vehicle context is now `Result<Option<RadarContext>>`. Bus open/read errors, missing bodies, JSON decode failures, wrong-host rows, and structurally invalid fixes defer the complete effect. They cannot masquerade as no context, clear `last_good`, publish false empty radar, or expose a prior-location frame.
- A successfully read offline/no-fix, stale/future-skewed fix, or valid point outside US radar coverage returns `Ok(None)`. Only that path may publish an honest empty snapshot; `last_good` is cleared only after the write succeeds.
- No-context publication is transition-based. After a successful empty write, repeated no-fix polls verify that the current Bus still contains the empty state and append no duplicate rows. A valid context resets suppression. A failed write leaves suppression unset, and a replaced index with no output row receives one corrected-forward empty publication.
- Metadata and tile I/O remains off-thread and bounded at six frames. Before publication, the worker fresh-opens the Bus and re-reads the exact vehicle context. Movement discards the old fetch and retries for the new point; genuine context loss publishes an empty retraction; read/open/decode failure remains effect-free.
- Fresh, degraded, and no-context publication now returns `io::Result`. Fresh state, paused `last_good`, empty-state clearing, and the successful refresh cadence advance only after the Bus write succeeds. Failed writes retain prior in-memory state and correct forward on retry.
- Shutdown remains selectable during retry sleeps and in-flight provider work.

## Focused verification

Farm host: machine194, `172.20.0.170`

Explicit slot: `MCNF_BUILD_HOST=172.20.0.170`, `MCNF_BUILD_SLOT=iem-radar-bus-r59`

The slot was created from `git archive HEAD`; only the owned worker source was overlaid. The initial cold build used:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=iem-radar-bus-r59 \
ssh -i ~/.ssh/mackes_mesh_ed25519 mm@172.20.0.170 \
  'cd magic-mesh-farm-iem-radar-bus-r59 && cargo test -p mackesd --lib \
  workers::iem_radar_overlay::tests::vehicle_read_or_decode_failure_is_effect_free \
  -- --exact --nocapture'
```

Result: pass. `1 passed; 0 failed; 4523 filtered out`. The clean slot emitted 256 existing crate warnings and no errors.

The built test binary was then run individually with `--exact --nocapture` for the other affected tests:

```text
workers::iem_radar_overlay::tests::captured_live_metadata_builds_six_exact_bounded_frames
workers::iem_radar_overlay::tests::vehicle_context_requires_fresh_same_host_us_fix
workers::iem_radar_overlay::tests::failed_publication_retains_state_and_corrects_forward
workers::iem_radar_overlay::tests::late_and_replaced_bus_recovers_in_the_same_worker
workers::iem_radar_overlay::tests::post_fetch_context_race_discards_old_tiles_and_retracts_on_loss
workers::iem_radar_overlay::tests::failed_refresh_retains_producer_time_and_publishes_paused_state
workers::iem_radar_overlay::tests::no_vehicle_fix_degraded_snapshot_is_present_and_retracts_prior_frames
workers::iem_radar_overlay::tests::shutdown_wins_while_metadata_is_in_flight
workers::iem_radar_overlay::tests::default_on_worker_publishes_degraded_snapshot_without_vehicle_fix
```

Results: all nine passed individually; each reported `1 passed; 0 failed; 4523 filtered out`. Together with the cold-build test, all ten exact affected tests passed during the initial r59 verification campaign.

Operational coverage:

- The late/replaced test begins with an unopenable Bus root, activates it without restarting the worker, publishes an external vehicle fix, replaces `index.sqlite`, and observes radar for a changed fix from the same worker. It also asserts canonical system fallback, the six-frame ceiling, and prompt shutdown.
- The read/decode test injects both a vehicle lane read error and malformed vehicle JSON and proves zero publication and zero `last_good` mutation.
- The write test proves failed fresh publication retains prior state, retry corrects forward, failed empty publication cannot clear `last_good`, and successful retry retracts it.
- The post-fetch race test changes vehicle location through a separate Persist handle while metadata/tile I/O is blocked. It proves the old-location result is never published, the new location is eventually published, all frames remain bounded, and subsequent genuine fix loss publishes an empty retraction.
- The repeated-no-fix runtime test proves sustained no-fix polling appends exactly one empty row, then atomically replaces the Bus index and proves the same worker publishes exactly one empty row into the replacement without further churn.

### Landing correction verification

After restoring no-context transition suppression, the final source was rebuilt in the same explicit machine194 slot. The following tests ran individually with `--exact --nocapture`:

```text
workers::iem_radar_overlay::tests::failed_publication_retains_state_and_corrects_forward
workers::iem_radar_overlay::tests::repeated_no_fix_polls_publish_once_and_replacement_retries
workers::iem_radar_overlay::tests::vehicle_read_or_decode_failure_is_effect_free
workers::iem_radar_overlay::tests::post_fetch_context_race_discards_old_tiles_and_retracts_on_loss
workers::iem_radar_overlay::tests::late_and_replaced_bus_recovers_in_the_same_worker
```

Results on the final source: all five passed; each reported `1 passed; 0 failed; 4523 filtered out`. The suppression test completed in 0.15 s, read/decode in 0.00 s, post-fetch race in 0.03 s, valid-context late/replacement recovery in 0.10 s, and failed-write correction in 0.00 s.

Scoped farm formatting and diff checks:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=iem-radar-bus-r59 \
ssh -i ~/.ssh/mackes_mesh_ed25519 mm@172.20.0.170 \
  'cd magic-mesh-farm-iem-radar-bus-r59 && \
  rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/iem_radar_overlay.rs'

ssh -i ~/.ssh/mackes_mesh_ed25519 mm@172.20.0.170 \
  'cd magic-mesh-farm-iem-radar-bus-r59 && \
  git diff --no-index --check /dev/null \
  crates/mesh/mackesd/src/workers/iem_radar_overlay.rs'
```

Results: pass. The authoritative local HEAD-relative `git diff --check -- crates/mesh/mackesd/src/workers/iem_radar_overlay.rs` also passed.

The first cold-build attempt found machine194 `/home` full and failed before compilation with `No space left on device`. Read-only inspection identified two completed, disposable slots from this same worklist drain (`health-reconciler-bus-r48` and `caltrans-overlay-bus-r54`, 6.6 GiB each). Only those two prior owned slots were removed; the r59 slot was preserved, free space recovered to 7.5 GiB, and verification then completed successfully.

## Hashes

Base HEAD: `f6eb3ec9a1a0219fa550c9c40cba4a6d5e01c860`

```text
704979d8c9db3c9a5a95ce36531540985ee3a4f59b2ec6fd1ee946bad9180cf7  crates/mesh/mackesd/src/workers/iem_radar_overlay.rs
a6271929e33a988071b1e2ab660066c43c298c267b6362023307ffdaae634bb1  scoped source patch against HEAD
```

The local and machine194 source hashes matched exactly.

## Blockers

None.
