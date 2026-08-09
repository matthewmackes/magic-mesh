# Airspace Bus publication recovery checkpoint (R52)

Date: 2026-08-09
Worklist: `WL-FUNC-017`, `WL-ARCH-009`

## Runtime correction

`AirspaceWorker` now resolves an explicit or current user Bus root for each
publication and falls back to `mde_bus::SYSTEM_BUS_ROOT`. Publication returns an
error instead of swallowing unavailable storage or a failed write. The worker
retains the already-completed bounded MG90 survey and retries that exact snapshot
with shutdown-aware 10 ms to 2 s backoff; it does not repeat the external survey
or record an unpublished snapshot as success. The no-source projection follows
the same retry contract, so starting before the Bus exists no longer leaves its
latest-wins topic permanently absent.

Survey freshness, stale-contact retraction, wire-size trimming, and explicit
offline/no-source semantics are unchanged. Bus publication is one durable topic
write; there is no cross-store transaction with the external MG90 survey.

## Focused farm verification

Host: machine 196 (`172.20.0.196`)
Slot: `airspace-bus-r52`

```text
MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=airspace-bus-r52 \
  install-helpers/xcp-build.sh cargo test -q -p mackesd \
  --features async-services --lib \
  workers::airspace::tests::worker_publishes_explicit_no_source_without_contacts \
  -- --exact --nocapture
# PASS: 1 passed; 0 failed; 4507 filtered out

MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=airspace-bus-r52 \
  install-helpers/xcp-build.sh cargo test -q -p mackesd \
  --features async-services --lib \
  workers::airspace::tests::failed_publication_retries_snapshot_without_reprobing \
  -- --exact --nocapture
# PASS: 1 passed; 0 failed; 4508 filtered out

rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/airspace.rs
# PASS after the one formatting-only line wrap recorded in the local source
```

The first test starts the no-source worker against a non-directory Bus root,
proves it remains supervised, then exposes a valid Bus and observes the typed
projection. The second counts external survey calls while publication is
blocked, proves the count remains one across retries, then observes publication
after storage recovery and prompt shutdown.

Machine 196's safety preflight initially found no active Cargo/rustc process but
less than 8 GiB free. Four explicit abandoned Aug-4 farm slots were removed;
active slots and non-farm paths were not touched.

Source SHA-256:
`b1a1a9ecb8e17fa5e1ee19073473fa04cbc3dce0c02badc836d75cf10702522e`.

No broad suite, local Cargo command, live-seat claim, WORKLIST edit, commit, or
push was used for this checkpoint.
