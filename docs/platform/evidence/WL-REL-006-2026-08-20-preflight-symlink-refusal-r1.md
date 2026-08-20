# WL-REL-006 release-input preflight symlink refusal — r1

Date: 2026-08-20 UTC  
Classification: focused helper regression evidence; not production release approval

## Gap closed

`release-input-preflight.sh` accepted a symlink for a governed input path when
called directly, even though `release-input-argv.py` rejects symlinked paths.
The preflight now refuses symlinked input files and the Maps source directory
before invoking any receipt verifier or materializer.

## Verification

- Local shell syntax probe:
  `bash -n install-helpers/release-input-preflight.sh
  install-helpers/test-release-input-preflight.sh` — PASS.
- Focused farm self-test, admitted slot
  `172.20.0.196 / rel006-preflight`:
  `bash install-helpers/test-release-input-preflight.sh` — PASS.
- The self-test covers valid six-role fixture admission, missing and
  substituted Maps inputs, substituted App VM manifest, mismatched bootc/App
  VM identity, verifier refusal, and the new symlinked Maps approval refusal.

This fixture evidence proves validator behavior only. It does not admit
production Maps/provider bytes, credentials, hardware, or release evidence.
