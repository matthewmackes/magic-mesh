# WL-CRIT-006 six-node artifact claim isolation (2026-08-09, r2)

## Production correction

`install-helpers/verify-six-node-topology.py` previously rejected one artifact
reused by different nodes but accepted one node's capture as proof of multiple
independent drills. The verifier now assigns each artifact path to exactly one
acceptance claim. The sole exception is the existing `failover` summary and
that node's detailed `recovery.failover` record, which intentionally describe
the same event. A hostile regression reuses `lh-1/join` as `lh-1/loss` and is
refused.

## Farm proof

- Build VM: `172.20.0.90`
- Slot: `crit006-artifact-claim-r1-20260809`
- `python3 install-helpers/verify-six-node-topology.py --self-test`: passed,
  2 positive and 18 negative cases.
- `python3 -m py_compile install-helpers/verify-six-node-topology.py`: passed.
- `install-helpers/release-evidence.sh --self-test`: passed the deterministic
  binding round-trip and fail-closed integration validation.
- Local scoped `git diff --check`: passed. The rsynced farm directory excludes
  `.git`, so an attempted farm `git diff --check` was inapplicable and made no
  correctness claim.
- Verifier SHA-256:
  `4a12ed270bb980e1f08d3d35c637abda848702d9789d0ba8180c7cd0fb164d70`.

This is a bounded verifier hardening checkpoint. It does not claim that the
live six-node/five-seat matrix has been executed or that WL-CRIT-006 is closed.
