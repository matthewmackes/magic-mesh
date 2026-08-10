# WL-CRIT-006 three-seat acceptance cap — 2026-08-10

## Outcome

Revision `73dbdc0af4d0e5d805f572de4050f700608e261a` makes the operator's
three-seat ceiling enforceable. The required release baseline is exactly Dell,
seat 15, and Surface. Eagle and T480 remain available only as explicitly
selected, non-gating inspections, and no collector activity can select more
than three physical seats.

The independently governed lighthouse roster remains exactly three nodes.
Accordingly, the six-node acceptance statement means three physical test seats
plus three lighthouses; it no longer implies five physical seats. Historical
five-seat evidence remains factual and is not rewritten as a current
requirement.

## Enforcement

- `release-gate-matrix.json` contains only `seat-dell`, `seat-seat15`, and
  `seat-surface` as required seat gates.
- The matrix verifier rejects an incomplete required roster, an implicit
  baseline, or promotion of an optional inspection into a required gate.
- The live collector defaults to the exact required baseline, fails when any
  baseline seat is unreachable, and labels Eagle/T480 runs as optional
  inspections rather than release passes.
- The active worklist's global test-seat and rollout locks prohibit validation,
  capture, chaos, recovery, acceptance, or rollout proof from requiring or
  exercising more than three physical seats in one activity.

## Focused verification

Machine 193 ran the matrix verifier and collector self-test against the exact
committed files: 14 hostile fixtures were rejected, 17 canonical gates were
validated, and the three-seat collector self-test passed. No broader test was
needed for this policy-only change.

File SHA-256 values at the committed revision:

- `install-helpers/release-gate-matrix.json`:
  `d9835f9b5c839fb658fedb4e81aaf9e9b639543e3638dc79b0dffac9378e795e`
- `install-helpers/verify-release-gate-matrix.py`:
  `75b9fcfe99a021ff1135cf99c980902729153bb89524a8ffb685990490d44355`
- `install-helpers/test-five-seat-core.py`:
  `50c7dc26b33227a42215444b9d92fd74423b9958852c65239db8bf93dc3dd655`

This checkpoint changes the required proof topology only. It does not claim
that release 31 has completed live seat or lighthouse acceptance.
