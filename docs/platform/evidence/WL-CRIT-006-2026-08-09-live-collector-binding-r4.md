# WL-CRIT-006 live collector binding — 2026-08-09

This bounded correction closes an evidence-integrity gap; it does not claim a
production promotion or complete six-node acceptance.

## Corrected-forward acceptance gap

`verify-six-node-topology.py --require-live` previously opened only the small
live-attestation marker. A caller could therefore replace a drill or recovery
artifact with arbitrary bytes, update its declared SHA-256, and retain a
plausible operator-authored `pass` record. The release verifier would bind those
bytes but would not prove that the production collector emitted the claim.

Live verification now requires every scenario and recovery artifact to use the
existing typed collector schema. Each claim is bound to its node, source
revision, collection time, event fields, candidate-manifest digest, package
payload digest, and installed runtime-binary digests. All three nodes of a role
must report the identical candidate payload. Artifact reuse is no longer
allowed, including the old synthetic failover alias; the production collector
already emits distinct scenario and recovery paths.

## Farm proof

Requested long-pole lane: BigBoy `172.20.0.130`, slot
`crit006-r4-20260809`.

- Python byte compilation passed for the collector and verifier.
- Complete collector self-test passed: 2 positive/redaction and 17
  negative/security cases.
- Complete verifier self-test passed: 2 positive and 20 negative cases.
- The new hostile cases reject rehashed arbitrary `pass` bytes and a split
  workstation candidate under one fleet revision.
- BigBoy lacked `jq`, so `release-evidence.sh --self-test` could not start on
  that image. The integration self-test passed on farm node `172.20.0.90`, slot
  `crit006-release-integration-r4-20260809`, where `jq` is available.
- Verifier SHA-256:
  `f7c38f2764caa0c94f31aa2fd7dc35f4c42226ea070187758d670d062a27287d`.

The named read-only `install-helpers/farm-topology.sh table` check reported all
5/5 build nodes reachable and 2/10 heavy slots active. It is build-farm
capacity evidence only; it is not represented as the required production
three-lighthouse/three-workstation proof. No package was deployed, and no host
was rebooted or mutated.

## Remaining production blockers

Closure still requires a current candidate manifest and direct live collection
from exactly three lighthouses and three workstations, the five-seat runtime
matrix, complete signed failure/recovery records, authoritative GitHub required
checks and final artifacts for that same revision, and a signed schema-5
production decision. BigBoy's missing `jq` is also farm-image drift for the
shell integration gate, not a production pass.
