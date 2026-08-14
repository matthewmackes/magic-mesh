# WL-FUNC-020 guest packaging contract farm gate

- Host/slot: `172.20.0.90` / `android-guest-packaging`
- Passed: `bash packaging/android/verify-contract.sh --self-test`;
  `bash packaging/android/verify-manifest.sh --self-test`;
  `bash packaging/android/verify-guest-payload.sh --self-test`; and
  `python3 packaging/android/test-produce-image-receipt.py`.
- Result: all four passed.
- Limitation: `test-guest-debs.sh` and
  `test-stage-guest-runtime-artifacts.sh` call `git rev-parse`/`git archive`;
  the farm workspace has no `.git` metadata by design, so those fixtures were
  not claimed as passed.
