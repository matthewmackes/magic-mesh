# WL-REL-001 source-freeze evidence (r2)

Date: 2026-08-15

The stale r1 source identity was replaced with the current clean, pushed
checkout:

```text
revision: 1dfe6906609d71da9ee2ce20c860912a09b32855
epoch: 1786813297
version: 12.1.6
tag plan: magic-mesh-v12.1.6
upstream: 1dfe6906609d71da9ee2ce20c860912a09b32855
git diff --quiet: 0
git diff --cached --quiet: 0
```

The authoritative receipt command was:

```text
bash install-helpers/source-revision-receipt.sh --repo .
1dfe6906609d71da9ee2ce20c860912a09b32855	1786813297
```

The branch was pushed before this evidence was recorded. No release artifact
build was attempted. WL-REL-001 S1 is complete; S3 remains blocked because the
private preflight argv and all required current-revision release-input
admissions are not available. Maps provider/live proof remains deferred to
WL-TEST-002 and is not treated as release-input approval.
