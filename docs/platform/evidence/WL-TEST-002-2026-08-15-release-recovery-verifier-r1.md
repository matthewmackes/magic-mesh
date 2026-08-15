# WL-TEST-002 release and recovery verifier evidence

Date: 2026-08-15

The reusable pre-release controls for the remaining post-release epic are
green, without claiming an installed release or live hardware/provider proof:

```text
install-helpers/verify-corrected-forward-recovery.py self-test
verify-corrected-forward-recovery: self-test passed 19/19

python3 install-helpers/verify-release-gate-matrix.py --self-test
verify-release-gate-matrix: self-test PASS (1 valid, 21 hostile fixtures rejected)
```

Exact-release admission, installed-seat behavior, provider activation,
direct-DRM/GUI captures, guest/device checks, and live corrected-forward drills
remain explicitly deferred until their signed artifacts and authorized inputs
are available. The two-seat cap is unchanged.
