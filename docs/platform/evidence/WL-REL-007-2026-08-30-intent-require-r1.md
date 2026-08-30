# WL-REL-007 live promotion requires admitted ReleaseIntentV1 — 2026-08-30

Classification: source coordinator increment. Dest-operator leftovers
stay parked. `production_admitted: false`. No dest invented. No live
lighthouse mutation.

Tree: `d829187cd` plus the require-admitted glue.

## Why this lands

Governance makes a signed, revision-bound `ReleaseIntentV1` the sole
release authorization. The contract helper could validate drafts, but
`mcnf-promotion-cycle.sh do` could still arm live DigitalOcean
lighthouse replacement with only `MCNF_ARM_LIVE=1`.

`release-intent.py --require-admitted` now refuses an unadmitted draft,
a missing dest path, or a checkout that no longer matches the bound
revision. `promote_do` calls that check before any scp/dnf. Default dest
is `/var/lib/mcnf-release/<HEAD>/intent.json`; override with
`MCNF_RELEASE_INTENT` only.

## Verification

```text
python3 automation/promotion/release-intent.py --self-test
# release-intent: ALL PASS

cargo metadata --format-version 1
```

Do not grind `cargo test --workspace`. Do not invent a signed intent.
Do not flip `production_admitted`.
