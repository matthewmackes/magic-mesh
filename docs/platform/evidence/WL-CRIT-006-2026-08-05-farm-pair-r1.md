# WL-CRIT-006 — release farm-pair binding slice (2026-08-05)

This record covers the schema-5 release-evidence binding improvement. It does
not claim a signed production promotion or complete six-node acceptance.

## Implemented

`install-helpers/release-evidence.sh` now constructs schema-5 farm job/slot
pairs together, sorts them by job identity without losing pairing, rejects
duplicate job or slot identities, and requires a referenced CI-gate status
record to identify the same job and host/slot pair. Schema-4 preview behavior
remains accepted only where the existing compatibility rule allows it.

## Verification

Focused farm run on `.90` (`wl-crit006-release-pair-r1`) passed the shell syntax
check and the complete deterministic self-test, including positive and hostile
cross-pair cases. ShellCheck reported only existing informational `SC2015`
notes. The temporary farm workspace was removed.

Production required-check publication, runtime HMAC verification/key
provisioning, drill-ledger topology/recovery evidence, live VDI/audio proof,
and a signed promotion bundle remain open gates.
