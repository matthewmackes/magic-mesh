# WL-FUNC-016 mesh expired-replay cleanup boundary (r17)

- Date: 2026-08-09
- Integrated base revision: `6fba280f9168dae2f430e6878b4b057c1c92e940` plus the scoped patch described here
- Production source: `crates/mesh/mackesd/src/workers/clipboard_sync/mesh.rs`
- Production source SHA-256: `e4403c07b28e2b84dce008996f7cc85e0901ae26d8169e7ecf434335321e0747`

## Defect and correction

The mesh receiver derived its replay `previous` generation before invoking the
ledger's expiry cleanup. An expired source/session replay marker could therefore
continue participating in admission for one more receive attempt. In
particular, an expired hostile marker at a very high generation rejected a
fresh, valid generation reusing the same bounded source/session identity, even
though that marker no longer owned the replay lane.

`receive_frame` now removes expired replay markers immediately after peer
authentication and before deriving the prior generation. Only markers whose
own expiry is at or before the injected receive time are removed. Unexpired
high-water marks retain their existing fail-closed replay behavior, and the new
generation is still recorded only after the canonical collaboration-lane write
succeeds.

## Hostile farm regression

Farm machine 9 build VM (`172.20.0.50`), slot
`clipboard-expired-replay-r1`:

```text
MCNF_BUILD_HOST=172.20.0.50 \
MCNF_BUILD_SLOT=clipboard-expired-replay-r1 \
install-helpers/xcp-build.sh \
  cargo test -p mackesd --features async-services \
  expired_hostile_generation_cannot_block_fresh_session_reuse -- --nocapture
```

Result: **PASS** — 1 passed, 0 failed, 0 ignored, 4,396 filtered out.

The regression plants an expired `u64::MAX` marker in the exact source/session
lane, submits a valid generation 1 envelope, and proves the receiver forwards
exactly one canonical envelope and replaces the stale marker with generation 1.
The build emitted pre-existing warning classes outside this scoped correction;
there were no test failures or blockers.
