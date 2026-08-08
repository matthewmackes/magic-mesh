# WL-UX-013 — expected-state health history boundary

Status: hostile history checkpoint complete; expected-state publishers,
transition history, recovery actions, and live five-seat proof remain
`Remaining`.

## Change

The health contract suite now covers a `Sleeping → Returned` transition at the
`u64::MAX` timestamp boundary and refuses an availability intent whose TTL
exceeds the governed maximum. This keeps expected absence informational while
preventing timestamp arithmetic and retention-window overflow from becoming a
false recovery or health result.

## Verification

Farm `.50` focused health lane:

```text
1 passed, 0 failed, 433 filtered out
```

`git diff --check -- crates/mesh/mackes-mesh-types/src/health.rs`: pass. No
live health publisher or recovery action was invoked.

## Source hash at capture

```text
39b920735f82630ffe051c8ed3b2e0c01cc2996f11d6101d969c5c91fdafbdbc  crates/mesh/mackes-mesh-types/src/health.rs
```
