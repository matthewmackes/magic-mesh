# WL-FUNC-019 manual-store root nofollow — r185

- Scope: Remote Sessions manual-source persistence refuses a symlinked store directory before reading, creating, or replacing its JSON store.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=func019-manual-root-nofollow-r185b install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::desktop_sources::tests::manual_store_rejects_a_symlinked_store_root -- --exact --nocapture`.
- Result: `1 passed; 0 failed; 4719 filtered out` on `.50`; the hostile target directory remained unchanged and was not read as the store root.
