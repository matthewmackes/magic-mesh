# WL-REL-007 / WL-REL-006 refuse REPLACE_* preflight leftovers — 2026-08-30

Classification: source increment. Dest-operator leftovers stay parked.
`production_admitted: false`. No dest invented. No `REPLACE_*` field was
filled.

Tree: `78978b2bc` plus the loader refusal.

## Why this lands

S7 still carries operator `REPLACE_*` tokens for Maps, App VM, and RPM
signer inputs. Those strings are not production paths. The argv loader
already required absolute existing files, but a clearer fail-closed
check now refuses any scalar that still contains `REPLACE_*` so a
template cannot be mistaken for an admitted preflight object.

## Verification

```text
python3 install-helpers/test-release-input-argv.py
# release-input-argv self-test: PASS

cargo metadata --format-version 1
```

Do not grind `cargo test --workspace`. Do not invent dests. Do not flip
`production_admitted`.
