# WL-FUNC-019 failed-service-probe launch admission — r164

- Revision: `0caa771f` (`stabilize failed service probe admission test`), including `b30d7dd8` (`revoke service launch after failed probe`).
- Scope: a configured service whose latest endpoint test failed remains visible as an unavailable resource, but its typed `launch` action is withheld even when the persisted configuration still says `enabled=true`.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func019-service-launch-r163 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::service_catalog::tests::failed_latest_test_revokes_launch_admission_even_when_enabled -- --nocapture`
- Result: `1 passed; 0 failed; 4704 filtered out` on seat 90.
