# WL-FUNC-017 offline cache timeline — r160

- Revision: `f781134d`
- Scope: impossible `last_access_ms < verified_at_ms` timelines fail closed and recover an empty index before renderer lookup.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=func017-offline-timeline-r160 install-helpers/xcp-build.sh cargo test -p mde-maps-location-egui --lib offline_cache::tests::impossible_access_timeline_recovers_empty_before_renderer_lookup -- --nocapture`
- Result: `1 passed; 0 failed; 310 filtered out` on BigBoy.

