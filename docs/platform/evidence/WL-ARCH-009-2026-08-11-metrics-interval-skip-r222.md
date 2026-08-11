# WL-ARCH-009 metrics interval recovery — 2026-08-11

- Scope: metrics exporter cadence under a slow blocking snapshot.
- Change: exporter intervals use Tokio `MissedTickBehavior::Skip`, preventing a delayed filesystem/SQLite export from bursting missed work and amplifying CPU/I/O pressure.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch009-export-interval-r222b install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::metrics_exporter::tests::exporter_interval_skips_slow_tick_backlog -- --exact --nocapture`
- Initial fixture hit an exact-deadline timing race; the fixture was corrected to assert half a cadence early. Production behavior was unchanged.
- Final farm result is recorded after the rerun.
