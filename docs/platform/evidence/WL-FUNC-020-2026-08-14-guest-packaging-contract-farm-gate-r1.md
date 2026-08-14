# WL-FUNC-020 guest packaging contract farm gate

- Hosts/slots: `.90` / `android-guest-packaging` for contract self-tests;
  BigBoy `.130` / `android-guest-packaging-bigboy` for full packaging fixtures.
- Passed: `bash packaging/android/verify-contract.sh --self-test`;
  `bash packaging/android/verify-manifest.sh --self-test`;
  `bash packaging/android/verify-guest-payload.sh --self-test`; and
  `python3 packaging/android/test-produce-image-receipt.py`.
- Result: all four passed.
- Full fixture result: after refreshing the tracked workspace lockfile and
  creating an isolated local Git snapshot, BigBoy passed
  `bash packaging/android/test-guest-debs.sh` (deterministic DEBs, verifier,
  and substitution hostile cases) and
  `bash packaging/android/test-stage-guest-runtime-artifacts.sh` (staging,
  verification, and hostile cases).
