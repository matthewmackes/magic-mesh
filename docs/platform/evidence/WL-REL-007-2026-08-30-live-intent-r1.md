# WL-REL-007 live smoke/audit/soak require admitted intent — 2026-08-30

Classification: source coordinator increment. Dest-operator leftovers
stay parked. `production_admitted: false`. No dest invented. No live
SSH or lighthouse mutation was run.

Tree: `a70cef22e` plus the live-stage require glue.

## Why this lands

`promote_do` already refused without an admitted `ReleaseIntentV1`.
`live-smoke`, `live-audit`, and `fd-soak` still reached live
lighthouses and Eagle without that authorization.

Those stages now call the same `require_admitted_release_intent`
check before any remote command. Missing dest path
`/var/lib/mcnf-release/<HEAD>/intent.json` fails closed.

## Verification

```text
bash -n automation/promotion/mcnf-promotion-cycle.sh
python3 automation/promotion/release-intent.py --self-test
cargo metadata --format-version 1
```

Do not grind `cargo test --workspace`. Do not invent a signed intent.
Do not flip `production_admitted`.
