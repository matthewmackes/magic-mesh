# WL-FUNC-018 / WL-ARCH-009 — Android Catalog Bus recovery (r57)

Date: 2026-08-09

Farm: BigBoy `172.20.0.130`, slot `android-catalog-bus-r57`

## Production semantics

- `AndroidCatalogWorker::new` no longer freezes an optional Bus root. Every pass resolves an explicit override, then the current user/environment root, then canonical `mde_bus::SYSTEM_BUS_ROOT` into one concrete path.
- Bus open, identity, retained-read, and publication failures are logged and retried by the same worker after the bounded one-second production poll; shutdown participates in that wait and remains prompt.
- The concrete `index.sqlite` device/inode identity is inspected on every successful open. A late index activates without daemon restart, and a replacement index resets only that index's import cursor, republishes durable current state, and then re-governs its retained import history from the beginning.
- `published_replay` and Bus identity commit only after durable last-good replay publication succeeds. Each import's `current` and cursor commit only after signature admission, durable cache persistence, and Bus publication all succeed. A failed publication therefore leaves the import unacknowledged for corrected-forward retry.
- `action/android-catalog/import/<host>` remains a durable replay-governed lane and is deliberately not tail-primed. Retained and replacement-index imports continue through the existing Ed25519 signature, signed payload digest, expiry, and monotonic revision authority; stale or untrusted rows cannot replace current state.
- Row-level commit boundaries preserve already completed revisions if a later row fails, preventing retry from rolling durable last-good authority backward.

## Focused hostile coverage

- `replay_and_import_publication_failures_preserve_state_for_retry`: injects a durable replay write failure and verifies `published_replay`, cursor, and Bus identity remain uncommitted; then injects an import publication failure after cache persistence, verifies cursor/current remain at revision 7 while durable recovery holds revision 8, and verifies corrected-forward publication on retry.
- `same_worker_recovers_late_and_replaced_bus_with_governed_replay`: starts against an unopenable root, admits a retained signed revision after the Bus appears, detects a replacement index and republishes revision 7, admits a forward revision 8 on that index, verifies canonical system fallback, and shuts down the original worker.

## Verification

Final-source focused commands on BigBoy:

```text
cargo test -p mackesd --lib --features async-services \
  workers::android_catalog::tests::replay_and_import_publication_failures_preserve_state_for_retry \
  -- --exact --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4529 filtered out

cargo test -p mackesd --lib --features async-services \
  workers::android_catalog::tests::same_worker_recovers_late_and_replaced_bus_with_governed_replay \
  -- --exact --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4529 filtered out
```

Formatting and scoped diff verification:

```text
rustfmt --edition 2021 --config skip_children=true --check \
  crates/mesh/mackesd/src/workers/android_catalog.rs
# exit 0 on BigBoy

git diff --check -- crates/mesh/mackesd/src/workers/android_catalog.rs
# exit 0
```

## Residual caveats

- Durable cache replacement and Bus publication are not one atomic operation in the current APIs. Cache persistence intentionally occurs first. If Bus publication fails, the cache can contain the newer signed catalog while the in-memory cursor/current remain old; same-process retry republishes the retained import, and process restart validates/replays the durable signed last-good catalog before the retained row is governed as stale.
- Equivalent state can be published more than once across a write-acknowledgement ambiguity or process crash. Signature/digest/revision authority makes the replay idempotent in meaning, but the Bus does not provide a transaction spanning its row and the filesystem cache.
- The first farm compile was blocked outside this ownership by a concurrent ignored `Result` in `workers/air_quality_overlay.rs`. The disposable r57 remote slot overlaid only that unrelated file from `HEAD` for isolated Android verification; no local concurrent file was edited or reverted.

## Hash

```text
beb379d8f591878fcde59fa9a12308570785c3c6a414f8a362502874daf83ea1  crates/mesh/mackesd/src/workers/android_catalog.rs
```

No WORKLIST edit, commit, or push was performed.
