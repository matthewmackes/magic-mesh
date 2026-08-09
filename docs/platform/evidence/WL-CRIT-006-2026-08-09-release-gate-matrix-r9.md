# WL-CRIT-006 S1 release gate matrix — 2026-08-09 r9

## Outcome

Added a fail-closed, machine-readable release plan bound to source revision
`46ea21b6f6c759fa6dcf09a28d62ba12040dc655`.

The matrix contains 19 explicit required gates: one authoritative GitHub check,
one farm gate, two role-specific package gates, five enrolled workstation seats,
three lighthouses, and seven named failure/recovery scenarios. Seat rows explicitly
cover runtime, GUI, network, audio, VDI, and package evidence; lighthouse rows cover
runtime, network, and package evidence.

The verifier rejects unknown fields and gates, duplicate gate IDs, duplicate
node/scenario claims, missing owner/command/evidence values, malformed or mismatched
revisions, incomplete/reordered rosters, unknown or incomplete categories, optional
required gates, and any gate not bound to the sole `source_revision` field.

## Files and digests

- `install-helpers/release-gate-matrix.json`
  - SHA-256: `debdfbf7c77b36d0513e90d65a7f2b21f9acc2933e07c47bc882615c5a22c4fd`
- `install-helpers/verify-release-gate-matrix.py`
  - SHA-256: `22df73cf086783620d6546856fa498e8c691bc01fdecbdfbc5c4010ba73651ad`

## Focused verification

Farm host: machine 196 (`172.20.0.196`)

Slot: `crit006-release-gate-matrix-r9`

Commands, after `xcp-build.sh sync` to the named slot:

```text
python3 -m py_compile install-helpers/verify-release-gate-matrix.py
install-helpers/verify-release-gate-matrix.py --self-test
install-helpers/verify-release-gate-matrix.py --expected-revision 46ea21b6f6c759fa6dcf09a28d62ba12040dc655 install-helpers/release-gate-matrix.json
```

Results:

```text
verify-release-gate-matrix: self-test PASS (1 valid, 12 hostile fixtures rejected)
verify-release-gate-matrix: PASS install-helpers/release-gate-matrix.json (19 explicit required gates)
```

Local `git diff --check` passed for the two implementation files and this evidence
file. No broad test, package build, live-seat action, commit, push, or worklist edit
was performed.

## Remaining blockers

This closes only the implementation/proof slice of S1. It does not execute or sign
the planned gates. GitHub/farm/package evidence, five-seat deployment, three-live-
lighthouse convergence, all failure injections, signed schema-5 aggregation, and
the production promotion decision remain required by WL-CRIT-006 S2-S6.
