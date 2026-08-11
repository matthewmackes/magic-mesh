# WL-FUNC-021 — direct media URL admission (r214)

- Scope: direct media URLs reject credentials, malformed authorities,
  unsupported schemes, whitespace/control, zero ports, fragments, and oversize.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func021-direct-media-url-admission-r214 install-helpers/xcp-build.sh cargo test -p mde-musicd --lib airsonic::tests::direct_media_url_admission_rejects_credentials_malformed_authority_and_scheme -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 243 filtered out`.
