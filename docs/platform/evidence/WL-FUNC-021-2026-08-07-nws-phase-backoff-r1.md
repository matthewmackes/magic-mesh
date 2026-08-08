# WL-FUNC-021 — NWS no-fix phase-desynchronized retry

Date: 2026-08-07

## Change

`crates/mesh/mackesd/src/workers/nws_alert_overlay.rs` now keeps the honest
empty degraded snapshot publication when the same-host MG90 fix is unavailable,
but no longer retries every seat on the same phase. The first retry in each
no-fix episode receives a deterministic host-derived offset of less than five
seconds. The offset is stable across restarts, is applied only once per episode,
and is reset after a successful NWS response. Later retries retain the bounded
5/10/20/40/60-second ladder; all retry delays are capped at 60 seconds (or a
shorter configured poll).

The first unavailable snapshot is still published immediately with an empty
alert list, cleared feed metadata, and the existing explicit vehicle-fix gap.
This changes retry scheduling only; it does not turn unavailable data into a
successful or stale alert projection.

## Verification

- Farm `.50`, slot `nws-phase-backoff-lib-r2`:

  ```text
  MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=nws-phase-backoff-lib-r2 \
    install-helpers/xcp-build.sh cargo test -p mackesd --lib \
    nws_alert_overlay --features async-services --locked -- --nocapture
  ```

  Result: **16 passed, 0 failed**, 4382 filtered.
- The new `no_fix_retry_is_bounded_and_phase_desynchronized` regression covers
  stable per-host phase selection, different host phases, the 60-second retry
  ceiling, backoff progression, and short test polls.
- Farm `.50` file-scoped `rustfmt --edition 2021 --check` passed for the owned
  worker file.
- An earlier `.90` full test-target attempt reached linking but refused to
  claim a result because the VM ran out of space (`ENOSPC`). The exact failed
  slot was removed and the focused library-only gate was rerouted to `.50`.

## Remaining proof

This is source and farm evidence only. No Dell or installed-seat runtime was
mutated or claimed by this scoped change; post-package CPU sampling and live
NWS recovery remain separate runtime proof.
