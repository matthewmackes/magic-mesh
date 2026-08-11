# WL-FUNC-016 — mesh replay expiry retention (r205)

- Scope: replay markers retain the maximum expiry across live generations and
  restart recovery, so a newer shorter-lived generation cannot bypass an older
  still-valid replay boundary.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func016-mesh-replay-expiry-r205-final install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::clipboard_sync::mesh::tests::restart_seed_retains_longest_expiry_across_newer_shorter_generation -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 4733 filtered out`.
