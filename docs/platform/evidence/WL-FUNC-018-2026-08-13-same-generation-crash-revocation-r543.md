# WL-FUNC-018 same-generation crash revocation — 2026-08-13 r543

## Production boundary

App VM runtime evidence is folded chronologically for the exact session,
application, VM, and node identity. A later `Failed` observation at the same
runtime generation now revokes an earlier `Connected` observation; a delayed
lower generation still fails closed. This prevents a crashed application from
retaining stale readiness without weakening rollback protection.

## Farm gates

- `.90`, slot `func018-crash-r543`: exact hostile regression
  `cargo test -p mackesd --features async-services workers::cloud::verbs::app_image::tests::later_same_generation_crash_revokes_connected_runtime -- --exact --nocapture`
  passed **1/1** with 4,985 filtered out.
- `.90`: `cargo clippy -p mackesd --features async-services --all-targets -- -D warnings`
  passed before handoff.
- `.170`: async-services `mackesd` build passed before handoff.
- `.90`: exact owned-file Rustfmt check passed before handoff.
- Scoped `git diff --check` passed.

The first `.50` attempt was terminated during compilation when `/home` reached
1.6 GiB free; its disposable owned workspace was removed and the node recovered
to 9.8 GiB. It is not counted as test evidence.

## Remaining acceptance

Pre-release work still includes the governed App VM image/runtime supply and
any remaining audio, persistence, reconnect, or cleanup gaps. Signed release,
physical display/input/audio, upgrade, crash/reconnect, and one-node acceptance
remain deferred and non-blocking until after the first full release.
