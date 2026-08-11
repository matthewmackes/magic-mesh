# WL-CRIT-006 — release gate identity bounds (r211)

- Scope: authenticated release `job_id`, `build_host`, and `build_slot` values
  over 255 characters are refused by the CI gate validator.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=crit006-release-identity-bound-r211 install-helpers/xcp-build.sh sync`; then the farm shell ran `./install-helpers/ci-gate.sh --self-test`.
- Result: `ci-gate.sh: self-test passed`.
