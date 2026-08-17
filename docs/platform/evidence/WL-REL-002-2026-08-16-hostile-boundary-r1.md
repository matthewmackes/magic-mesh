# WL-REL-002 hostile phase-boundary evidence — 2026-08-16

Command:

```text
install-helpers/test-run-first-full-release.sh
```

Result: `PASS`.

The suite exercised the unsigned operator handoff refusal and the verified
seven-role resume path. It also confirmed that promotion remains forbidden in
both fixture paths. This proves the local prepare/resume phase boundary only;
it does not admit a production revision or prove the three-RPM farm lane.
