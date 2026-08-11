# WL-FUNC-019 service-record freshness — 2026-08-11

- Scope: catalog republication cannot renew the health lease of an old service observation.
- Hostile boundary: stale, zero, or future `last_seen_ms` forces retained rows to `Stale`/degraded even when their replayed source claims `Up`.
- Focused gate: `cargo test -p mackesd workers::service_catalog::tests::replayed_service_rows_cannot_regain_available_health_from_catalog_publication -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 2, admitted with 12,554,940 KiB disk and 10,274,708 KiB memory available.
- Result: **PASS**, 1 passed, 0 failed, 4,871 filtered out.
- Remaining boundary: live Bus stale/future replay through installed catalog consumers remains.
