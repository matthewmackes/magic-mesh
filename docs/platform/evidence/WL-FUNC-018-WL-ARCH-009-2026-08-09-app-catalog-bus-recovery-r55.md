# WL-FUNC-018 / WL-ARCH-009 — App Catalog Bus recovery (r55)

Date: 2026-08-09

Farm: BigBoy `172.20.0.130`, slot `app-catalog-bus-r55`

## Production semantics

- `AppCatalogWorker::new` no longer freezes a missing optional Bus root. Every pass resolves an explicit override, then the current user/environment resolver, then canonical `mde_bus::SYSTEM_BUS_ROOT` into one concrete path.
- Bus open and live-index inspection failures are logged and retried by the same worker after the bounded one-second production poll, with shutdown included in the wait.
- The complete import transaction is cloned before each pass: Bus identity, startup-recovery attempt, import cursor, current catalog, durable watermark, and edge-triggered status. It commits only after activation, retained-command reads, projection writes, and required status writes all succeed.
- A failed Bus open/read/write therefore advances none of `cursor`, `current`, `watermark`, `last_status`, `recovery_attempted`, or the remembered Bus identity. The next poll retries from the prior committed state.
- Replacement `index.sqlite` identity is detected per pass. The same worker republishes its current admitted projection (or durable expired-catalog retraction) and status before admitting further imports on the replacement Bus.
- `action/app-catalog/import/<host>` remains a durable replay-governed lane rather than a transient tail-primed command lane. Retained catalogs are still admitted only through the existing Ed25519 signer check and persisted catalog-id/revision/content-digest authority: exact replay is idempotent, rollback/conflict/identity changes remain refused, and a newer valid signed revision remains forward work.

## Focused hostile coverage

- `status_publication_failure_rolls_back_pass_state_and_retries`: injects a status write failure after projection publication, verifies every transaction field including `recovery_attempted` and Bus identity remains uncommitted, then verifies the same retained signed import succeeds on retry.
- `same_worker_recovers_late_and_replaced_bus_with_governed_replay`: starts against an unopenable root, activates a late Bus containing a retained signed catalog, verifies governed admission, atomically replaces the Bus index, verifies projection rehydration, admits a forward signed revision, and shuts down the original worker.
- Existing signed replay/rollback, projection-failure, persistence, startup-recovery, hostile-payload, and edge-trigger status tests remained green in the scoped module run.

## Verification

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=app-catalog-bus-r55 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  --features async-services workers::app_catalog -- --nocapture

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 4504 filtered out
```

After the final whole-pass transaction boundary was added, the exact operation-impacting tests were rerun:

```text
cargo test -p mackesd --lib --features async-services \
  status_publication_failure_rolls_back_pass_state_and_retries -- --nocapture
test result: ok. 1 passed; 0 failed

cargo test -p mackesd --lib --features async-services \
  same_worker_recovers_late_and_replaced_bus_with_governed_replay -- --nocapture
test result: ok. 1 passed; 0 failed
```

Formatting and scoped diff verification:

```text
rustfmt --edition 2021 --config skip_children=true --check \
  crates/mesh/mackesd/src/workers/app_catalog.rs
# exit 0 on BigBoy

git diff --check -- crates/mesh/mackesd/src/workers/app_catalog.rs
# exit 0
```

## Residual caveats

- Projection and status are separate Bus rows and cannot be atomically committed by the current Bus API. If projection succeeds and status fails, the worker intentionally leaves transaction state uncommitted and may publish a duplicate equivalent projection on retry; it never acknowledges the import or claims successful in-memory convergence early.
- Durable last-good persistence still precedes Bus publication. This is the existing crash window and the reason retained imports remain replay-governed: restart recovery validates the signed persisted catalog and folds exact replay/rollback rules before advancing authority.
- The first farm compile attempt was blocked outside this ownership by a concurrent duplicate `io_other` definition in `workers/caltrans_camera_overlay.rs`. The isolated r55 farm slot overlaid that unrelated file from `HEAD`; no local concurrent file was edited or reverted.

## Hash

```text
7fb69355081326bbbf520eb5b115e445e8df305322f1b881d6f5a6436dc41815  crates/mesh/mackesd/src/workers/app_catalog.rs
```

No WORKLIST edit, commit, or push was performed.
