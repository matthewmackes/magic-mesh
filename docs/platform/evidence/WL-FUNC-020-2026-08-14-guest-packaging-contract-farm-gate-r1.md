# WL-FUNC-020 guest packaging contract farm gate

- Host/slot: `172.20.0.90` / `android-guest-packaging`
- Passed: `bash packaging/android/verify-contract.sh --self-test`;
  `bash packaging/android/verify-manifest.sh --self-test`;
  `bash packaging/android/verify-guest-payload.sh --self-test`; and
  `python3 packaging/android/test-produce-image-receipt.py`.
- Result: all four passed.
- Full fixture attempt: after creating an isolated local Git snapshot in the
  farm workspace, `test-guest-debs.sh` and
  `test-stage-guest-runtime-artifacts.sh` reached the archived source build but
  failed before producing artifacts: `cargo build --locked` reported that it
  could not update the archived `Cargo.lock`. The snapshot contains
  `Cargo.lock`; this is recorded as a reproducible Cargo lock/dependency
  resolution blocker, not as package proof.
