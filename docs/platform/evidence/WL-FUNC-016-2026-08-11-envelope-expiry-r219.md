# WL-FUNC-016 evidence — materialization envelope expiry (r219)

- Scope: VDI Files clipboard one-use materialization.
- Change: descriptor authorization and command retention expire at the earlier
  of the capability lease and envelope expiry, preventing a still-valid lease
  from outliving the signed envelope.
- Farm host: `172.20.0.130` (BigBoy).
- Farm slot: `func016-materializer-envelope-retention-r219-final`.
- Gate:
  `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=func016-materializer-envelope-retention-r219-final install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::transfers::clipboard_materializer::tests::exact_files_command_releases_one_verified_descriptor_once -- --exact --nocapture`
- Result: `1 passed; 0 failed; 4747 filtered out`.
