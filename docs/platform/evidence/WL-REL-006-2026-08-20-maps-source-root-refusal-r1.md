# WL-REL-006 Maps source-root refusal — r1

Date: 2026-08-20 UTC  
Classification: focused helper regression evidence; not production release approval

## Gap closed

The release-input preflight already refused symlinked governed files, but its
self-test did not exercise a symlink substituted for the Maps tile source
directory. The regression test now proves that substitution is refused before
receipt verification, materialization, or any release-build mutation.

## Verification

- Farm topology: `install-helpers/farm-topology.sh table` — 5/5 nodes up,
  6/10 heavy slots active.
- Canonical farm helper sync and focused gate:
  `MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=rel006-preflight
  install-helpers/xcp-build.sh sync`, followed by the synced
  `test-release-input-preflight.sh` — PASS.
- The farm self-test passed valid six-role fixture admission, missing and
  substituted Maps inputs, symlinked Maps approval, symlinked Maps source root,
  substituted App VM manifest, mismatched bootc/App VM identity, verifier
  refusal, and release-order checks.

This evidence proves validator behavior only. It does not admit production Maps
provider bytes, credentials, hardware, or the candidate-bound release input
set. WL-REL-006 remains blocked until WL-REL-001 supplies the clean candidate
revision and the real Maps/App VM/bootc/RPM input receipts are materialized and
admitted against it.
