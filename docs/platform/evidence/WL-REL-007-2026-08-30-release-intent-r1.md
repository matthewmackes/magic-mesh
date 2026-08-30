# WL-REL-007 S1 release-intent contracts — 2026-08-30

Classification: source/cargo coordinator increment. Dest-operator
admission and a signed `ReleaseIntentV1` on protected `master` remain
parked leftovers. `production_admitted: false`. No dest invented.

Tree: `601603815` plus this helper. Official unit:
`cargo metadata --format-version 1`.

## Why this lands

S1 needed typed `ReleaseIntentV1` and `ReleaseStateV1` next to the
existing `ReleaseStageReceiptV1` journal. Drafts bind version `13.0.0`,
a 40-character source revision, the six roles, Dell / Seat 15 / Surface
/ LH1–LH3, credential names only, and a bounded destructive scope.
Hostile paths refuse Cuttlefish roles, invented dest fields, short
signatures, `production_admitted` flips, stale state generations, and
`passed` without an admitted signed intent.

## Verification

```text
python3 automation/promotion/release-intent.py --self-test
# release-intent: ALL PASS

cargo metadata --format-version 1
# 961 packages; workspace /root/magic-mesh
```

Do not grind `cargo test --workspace`. Do not flip
`production_admitted`. Signed admission stays dest-operator leftover.
