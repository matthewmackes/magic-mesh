# WL-CRIT-006 — Workloads gate-matrix revision rebasing (r507)

## Result

The mandatory Workloads RPM transaction gate now uses the single bounded
`MCNF_WORKLOADS_RPM_TRANSACTION_EVIDENCE` input for its validation command.
The matrix continues to bind the exact canonical evidence filename to
`source_revision`; rebasing a candidate matrix no longer leaves the command
pointing at an earlier revision.

The verifier requires the exact parameterized command, the canonical
revision-bound `workloads-rpm-transaction.json` evidence filename, and the full
dependency, payload, repository transaction, upgrade, and owner-handoff pass
condition. Arbitrary commands and stale or cross-wired evidence claims remain
fail closed.

## Reproduction

Before this change:

```text
verify-release-gate-matrix: FAIL: gates[2].command must validate the revision-bound Workloads RPM transaction evidence
release-evidence: production gate manifest is not the complete canonical matrix for the source revision
```

The standalone matrix self-test passed, proving the mismatch was in the
release-evidence revision-rebase fixture rather than missing mandatory gate
membership.

## Verification

Farm host `172.20.0.90`, slot `crit006matrix`:

```text
python3 -m py_compile install-helpers/verify-release-gate-matrix.py
python3 install-helpers/verify-release-gate-matrix.py --self-test
python3 install-helpers/verify-release-gate-matrix.py install-helpers/release-gate-matrix.json
install-helpers/release-evidence.sh --self-test
```

```text
verify-release-gate-matrix: self-test PASS (1 valid, 21 hostile fixtures rejected)
verify-release-gate-matrix: PASS install-helpers/release-gate-matrix.json (18 explicit required gates)
release-evidence: self-test passed (deterministic binding round-trip + fail-closed validation)
```
