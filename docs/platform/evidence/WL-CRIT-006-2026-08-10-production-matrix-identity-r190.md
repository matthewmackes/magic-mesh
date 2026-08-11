# WL-CRIT-006 production matrix identity gate — 2026-08-10

This checkpoint closes one production-release boundary only. It does not claim
that a candidate is signed, promoted, or accepted on the live three-seat and
three-lighthouse fleet.

## Production gate

`install-helpers/release-evidence.sh validate` already bound the gate-manifest
bytes into the evidence envelope, but a production `pass` could still point at
an arbitrary structurally valid JSON file. The validator now invokes
`verify-release-gate-matrix.py --expected-revision <source_commit>` for every
production pass. This requires the complete canonical roster and gate IDs,
revision-bound evidence filenames, explicit required gates, and the exact
source revision before promotion can be accepted.

The check is production-only: preview and `not-promoted` evidence may continue
to record bounded fixtures and named unavailable live proof without pretending
that they are a promotion.

## Farm proof

The focused command ran on build VM `172.20.0.90`, slot
`crit006-production-matrix-identity-r190`:

```text
MCNF_BUILD_HOST=172.20.0.90
MCNF_BUILD_SLOT=crit006-production-matrix-identity-r190
bash -n install-helpers/release-evidence.sh
python3 install-helpers/verify-release-gate-matrix.py --self-test
install-helpers/release-evidence.sh --self-test
CRIT006_FARM_RESULT:0
```

Result: shell syntax passed; the release-matrix verifier reported `1 valid, 16
hostile fixtures rejected`; the release-evidence self-test passed, including
the canonical matrix acceptance and hostile source-revision refusal. The
explicit remote command returned `CRIT006_FARM_RESULT:0`.

## Live-proof limits

No physical seat, lighthouse, signed artifact, GitHub required-check run, or
production deployment was performed here. WL-CRIT-006 remains `Remaining`
until the complete signed evidence bundle and live acceptance matrix exist.
