# WL-FUNC-017 location provenance revalidation — 2026-08-11

- Scope: atmospheric refresh admission binds location mode and the complete effective-location identity, not only generation and coordinates.
- Hostile boundary: a provider fetch whose same-generation location is replaced with different provenance is discarded before publication.
- Focused gate: `cargo test -p mackesd workers::weather_atmosphere::tests::same_generation_location_provenance_substitution_discards_snapshot -- --exact --nocapture`.
- Farm: BigBoy `172.20.0.130`, slot 2, admitted with 13,788,580 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,853 filtered out.
- Remaining boundary: live location/provider replacement and installed Maps proof remain.
