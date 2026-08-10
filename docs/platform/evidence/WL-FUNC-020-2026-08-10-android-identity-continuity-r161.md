# WL-FUNC-020 Android catalog identity continuity — r161

- Revision: `b98401ac`
- Scope: a higher-revision signed Android import cannot switch the established catalog identity; the terminal refusal advances the cursor without publishing.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func020-catalog-identity-r161 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::android_catalog::tests::trusted_higher_revision_cannot_switch_catalog_identity -- --nocapture`
- Result: `1 passed; 0 failed; 4702 filtered out` on seat 90.

