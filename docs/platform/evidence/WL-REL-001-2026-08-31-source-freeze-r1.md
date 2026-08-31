# WL-REL-001 source freeze — dest-cut `42035dcbd` — 2026-08-31

Classification: source freeze of already-selected dest-cut on protected
`master`. Not a tag. Not publication. Not live-seat proof.
`github-required` is required on `master` and has not yet passed.

Operator 2026-08-31: authorized dest-cut freeze, Maps
`production_admitted`, Surface selected dest pin, `github-required`,
RPM signer inspect, and S4 agreement.

## Frozen identity

```text
revision: 42035dcbd76b03b8323399892052b21a96e2e233
epoch:    1788153988
version:  13.0.0
tag plan: magic-mesh-v13.0.0
upstream: origin/master (fast-forward 70c69c14d..42035dcbd)
```

`install-helpers/source-revision-receipt.sh --verify` returned the same
revision and epoch. S7 `release-input-preflight` already passed at this
SHA. Dest receipts were not rebound. Drain-branch documentation after
this SHA is not the freeze tree.

## Dest-operator leftovers closed without inventing dests

| Leftover | Result |
|---|---|
| Maps dest `6d01a543…` | Operator-admission sidecar on BigBoy plus private `/root/mcnf-private/maps-operator-admission-42035dcbd.json`. Canonical MBTiles not replaced. |
| Surface `bootc_base` | Private dest pin of selected `quay.io/fedora/fedora-bootc@sha256:3a5e74e6…`. In-tree `surface-stack.f44.json` stays blocked (unsigned Surface RPMs). |
| RPM signer | BigBoy inspect of dest-cut receipt matched fingerprint `06B1C27EA0E08A225155EB3314018AA1497DDC7C`. |
| `github-required` | Ruleset `Release github-required` (id `21919059`) requires context `github-required` on `master`. Workflow `33381446101` dispatched at this SHA; not yet pass. |

Helpers still refuse to self-mark `production_admitted`. Kiron
`--source --expected-source-revision 42035dcbd` passed.

Do not grind `cargo test --workspace`. Do not tag `magic-mesh-v13.0.0`
here (`WL-REL-005`).
