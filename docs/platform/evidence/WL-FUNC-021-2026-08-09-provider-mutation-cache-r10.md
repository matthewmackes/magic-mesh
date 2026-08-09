# WL-FUNC-021 — Provider mutation cache reconciliation (2026-08-09)

## Production change

Successful Airsonic/OpenSubsonic star, unstar, bookmark, and playlist mutations
now invalidate the exact durable metadata views they supersede. This prevents a
later provider outage from replaying a pre-mutation Starred, Bookmarks, playlist
roster, or playlist-detail projection.

Invalidation happens only after the provider acknowledges the mutation. A local
removal failure is not hidden: the client returns `AirsonicError::LocalState`
with explicit wording that the provider mutation was already acknowledged. The
stale fallback can still exist in that failure case, so callers receive an
honest partial-effect result instead of a false success or provider rejection.

Source:

```text
ca3f1a83db08e67c6f59a4cc7175162f5c49bc1660f1103e32a8aab9784479ec  crates/services/mde-musicd/src/airsonic.rs
```

## Focused farm verification

Machine 193 (`172.20.0.90`), slot
`func021-provider-cache-invalidation-r10`:

```text
MCNF_BUILD_HOST=172.20.0.90 \
MCNF_BUILD_SLOT=func021-provider-cache-invalidation-r10 \
install-helpers/xcp-build.sh cargo test --locked -p mde-musicd \
  airsonic::tests::acknowledged_provider_mutation -- --nocapture
```

Result: **2 passed, 0 failed; 236 filtered out**. The regressions cover exact
Starred/Bookmarks/playlist invalidation after provider acknowledgement and an
unremovable cache entry returning visible `LocalState` while preserving the
fact that the provider side effect already occurred.

Exact-file `rustfmt --edition 2021 --check` passed on machine 193 after syncing
the same slot. Scoped `git diff --check` passed locally. No broad or unrelated
test suite was run.

## Boundary

This is provider/catalog consistency implementation and farm evidence. It does
not claim live provider-loss continuity, physical renderer acceptance, or the
remaining two-seat handoff proof.
