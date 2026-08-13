# WL-UX-011 device-control schema admission — r495

Date: 2026-08-13

The reachable privileged `device_control` worker now rejects every ordinary
request whose schema version is not the exact supported typed contract before
inventory admission, capability verification, or hardware mutation. Previously,
the cancellation path enforced this boundary but an otherwise correctly signed
ordinary request carrying a future or incompatible schema could reach a fixed
sysfs or helper mutation.

The hostile regression signs an unsupported-schema USB-disable request with the
canonical action authorizer and proves that the simulated kernel `authorized`
control remains unchanged.

Farm evidence:

- `.170`, slot `ux011-device-schema-clippy-r495`: `cargo clippy -p mackesd
  --lib -- -D warnings` passed.
- `.50`, slot `ux011-device-schema-fmt-r495`: `rustfmt --edition 2021 --check
  crates/mesh/mackesd/src/workers/device_control.rs` passed against the final
  file. The first check exposed pre-existing formatting drift inside this same
  exclusively assigned file; it was mechanically normalized before the green
  rerun.
- `.170`, slot `ux011-device-schema-clippy-r495`: `cargo test -p mackesd --lib
  workers::device_control::tests::signed_unsupported_request_schema_never_reaches_hardware
  -- --exact --nocapture` passed 1/1 with 4,937 filtered. The first compile
  attempt failed before test execution on concurrent duplicate derive/serde
  attributes in `ipc/files.rs`; the green rerun used a disposable farm-only
  correction and did not edit or commit that unrelated Files scope.

This is implementation evidence, not physical-fleet acceptance. Remaining
WL-UX-011 acceptance is the complete provider/control coverage matrix and the
deferred post-release physical seat matrix proving safe controls, failure
feedback, sleep/rejoin behavior, and supported hardware coverage.
