# WL-TEST-002 release-input boundary evidence

Date: 2026-08-15

The release-input and first-release phase-boundary controls pass without
claiming that a signed release has been produced:

```text
install-helpers/test-release-input-preflight.sh
release-input-preflight: self-test PASS (missing or mismatched receipts stop before build command)

install-helpers/test-run-first-full-release.sh
test-run-first-full-release: hostile phase-boundary suite passed
```

The suite also verified mandatory-input admission and bootc receipt binding,
and rejected unsigned operator handoff and incomplete resumed output. Exact
signed-release identity, installed-seat behavior, providers, hardware, and
live recovery remain post-release obligations. The two-seat cap is unchanged.
