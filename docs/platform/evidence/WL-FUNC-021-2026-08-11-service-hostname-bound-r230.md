# WL-FUNC-021 service registration hostname bound — 2026-08-11

- Scope: local media-service registration reads `/etc/hostname` through a regular-file reader capped at 255 bytes before validation.
- Farm: BigBoy `172.20.0.130`, slot `func021-service-hostname-bound-r230b`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=func021-service-hostname-bound-r230b install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::service_catalog::tests::oversized_hostname_input_fails_closed_before_validation -- --exact --nocapture`
- Result: PASS, 1 passed, 0 failed.
