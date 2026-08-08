# WL-CRIT-006 — required-check artifact binding flow (2026-08-05)

The authenticated farm gate now accepts one final release binding containing
the full source revision, exact sorted artifact descriptors, and its own exact
job/host/slot identity. Publication rejects malformed, duplicate, unsorted,
oversized, symlinked, or mismatched inputs, appends one digest line to the gate
log under its writer lock, and refreshes the status artifact's authenticated log
digest. Release validation requires that digest to match the candidate's exact
artifact set, preventing reuse of one green required check for different bytes.

`release-evidence.sh write-binding` closes the producer-side manual JSON gap. It
hashes caller-supplied final artifacts, re-verifies the green CI status/log,
copies its exact farm identity, and atomically writes the canonical schema-1
input consumed by `ci-gate.sh bind-release`.

## Verification

- Farm `.90`, slot `wl-crit006-ci-publisher-r2`:
  `bash install-helpers/ci-gate.sh --self-test`.
- Result: `ci-gate.sh: self-test passed — policy failures, authenticated status,
  and fail-closed release binding propagate`.
- Farm `.90`, slot `wl-crit006-release-orchestration-r3`:
  `bash -n install-helpers/release-evidence.sh &&
  install-helpers/release-evidence.sh --self-test`.
- Result: `release-evidence: self-test passed (deterministic binding round-trip
  + fail-closed validation)`.

The second self-test covers the complete local sequence `write-binding` →
`ci-gate.sh bind-release` → release-evidence write/validate and rejects changed
descriptors and status evidence.

## Remaining acceptance edge

The GitHub release workflow still has to invoke this sequence after its final
artifact set exists and upload the refreshed status/log with the candidate.
This evidence does not claim that workflow invocation or a signed production
promotion bundle exists.
