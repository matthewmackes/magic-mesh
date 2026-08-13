# WL-CRIT-006 — explicit RPM signer admission (r527)

Date: 2026-08-13

The Workstation/App VM RPM supply verifier and Browser/Lighthouse candidate
verifier now accept an explicit expected full signing fingerprint. Verification
already authenticated the RPM against the governed release key and bound that
resolved fingerprint to the immutable candidate manifest; the new assertion
also requires it to equal the release collector's independently supplied
`signing_identity`. A mismatched or malformed fingerprint fails before the
candidate is admitted. Existing image-builder callers remain compatible when
they are not acting as the final release collector.

## Farm evidence

- `.50`, slot `crit006-app-signer-r527`:
  `bash packaging/app-vm/verify-rpm-supply.sh --self-test` passed, including the
  matching explicit fingerprint and hostile mismatched-fingerprint cases.
- `.50`, slot `crit006-lighthouse-signer-r527`:
  `python3 packaging/browser-vm/test-produce-lighthouse-rpm-candidate.py`
  passed, including matching and mismatched explicit signer verification.
- Strict ShellCheck passed for `packaging/app-vm/verify-rpm-supply.sh`.
- Python compilation and tabnanny passed for the Lighthouse producer/verifier
  and its hostile integration suite.
- Local Bash syntax and `git diff --check` passed.

## Remaining release-output plan prerequisites

- Governed Server RPM candidate production and explicit-signer verification.
- Explicit source-revision admission for the Browser qcow2 verifier.
- A role-appropriate App VM qcow2 manifest verifier.
- The canonical bounded seven-role collection-plan producer.
