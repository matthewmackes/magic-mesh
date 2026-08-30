# WL-FUNC-023 source close — 2026-08-30

Classification: source/cargo close. Dest-gated live leftover and
Construct Health Fix remain `WL-TEST-003` after a testing Beta.

Tree: `519c415bc` (`fix: pin upgrade packages before walking a blocked
plan`). `production_admitted: false`. No dest invented. Dest-cut
`bc14a22d7` is not a testing Beta.

## Why this closes

S1–S18 source is in-tree: typed lifecycle contracts, one `mackesd`
declared-step executor (`lifecycle_step.rs`), capsules, join dest
writers, ONBOARD nags, fleet wipe/reconnect, ResetAndOnboard leftover
refuse, and Upgrade pin+preflight+resume. Official crate gate passed at
the committed HEAD. Turnkey dest install/wipe and the Health Fix click
stay on `WL-TEST-003`.

## Farm (one crate, clean HEAD)

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=147
./install-helpers/xcp-build.sh cargo test -p mackesd
```

Admission: 9,776,084 KiB free on BigBoy `.130` (required 8,388,608 KiB).
Result: **5187 passed, 0 failed, 1 ignored** in the library suite, plus
passing crate bins/integration; exit 0.

Do not grind `cargo test --workspace` for this close.

Live leftover: `WL-FUNC-023-2026-08-25-destcut-bc14a22d7-r1.md`.
S18 index: `WL-FUNC-023-2026-08-20-s18-evidence-index.md`.
