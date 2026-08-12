# WL-ARCH-009 — metrics slow-export recovery gates (2026-08-12)

## Scope

This gate verifies the metrics exporter’s slow-tick recovery contract: missed
Tokio interval ticks are skipped rather than replayed as a burst.

## Farm evidence

Independent farm lanes were used:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch009-metrics-test-20260812a MCNF_BUILD_SHAPE=small install-helpers/xcp-build.sh cargo test -p mackesd --locked metrics_exporter::tests::exporter_interval_skips_slow_tick_backlog
Finished `test` profile in 12m 14s
1 passed, 0 failed; all filtered targets passed with 0 failures

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch009-metrics-clippy-20260812a MCNF_BUILD_SHAPE=small install-helpers/xcp-build.sh cargo clippy -p mackesd --locked --lib
Finished `dev` profile in 2m 12s
exit 0; warnings only
```

The clippy run produced existing warning inventory only; no denied lint or
compilation error occurred.

## Acceptance status

The focused implementation gate is green. Fleet/live acceptance and release
proof remain separate criteria and are deferred until the first full release
under the active release policy.
