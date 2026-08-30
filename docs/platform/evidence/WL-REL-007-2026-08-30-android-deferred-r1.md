# WL-REL-007 S4 Android/Cuttlefish deferred projection — 2026-08-30

Classification: source increment. Dest-operator leftovers stay parked.
`production_admitted: false`. No dest invented. Android is not launched.

Tree: `6f36d82a9` plus the lifecycle-view deferral. Official REL-007 unit
remains `cargo metadata --format-version 1`. Focused proof is
`cargo test -p mackes-mesh-types`.

## Why this lands

Release helpers already refuse `--cuttlefish-*` / `--android-*` and
Cuttlefish-bearing production objects (REL-006 S4). The shared
`LifecycleSessionView` still treated any warning as `ReadyWithWarnings`
and listed withdrawn capabilities as unavailable.

The view now:

- always renders `android: Deferred` on GUI and TUI capability lines
- drops android/cuttlefish names from the unavailable-capability list
- ignores those warnings when classifying readiness

A planted `capability unavailable: android` / `cuttlefish` pair stays
`Ready` with the Deferred line. KVM and other production warnings still
withdraw those capabilities.

## Verification

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=148
./install-helpers/xcp-build.sh cargo test -p mackes-mesh-types --lib lifecycle_view
```

Admission: 10,633,120 KiB free on `.50` (required 8,388,608 KiB).
Result: **18 passed, 0 failed**, 541 filtered out, exit 0.

Local official unit: `cargo metadata --format-version 1` (961 packages).

Do not grind `cargo test --workspace`. Do not flip `production_admitted`.
