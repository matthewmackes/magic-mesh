# WL-FUNC-019 manual-store admission — r193e

- Scope: Remote Sessions validates persisted manual-source records with the same bounded host, port, and name grammar as request admission; invalid stores fail closed without partial publication.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func019-manual-store-admission-r193e-final install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::desktop_sources::tests::manual_store_rejects_records_that_bypass_request_admission -- --exact --nocapture`.
- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 4727 filtered out` on `.90`.
