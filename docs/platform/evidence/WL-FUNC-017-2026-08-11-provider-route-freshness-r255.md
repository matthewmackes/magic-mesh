# WL-FUNC-017 provider route freshness — 2026-08-11

- Scope: the installed `NavigationWorker` now requires a provider result's
  calculation timestamp to be at or after the initiating route request. A
  provider cannot relabel cached geometry as a fresh route merely by copying
  the current request identity; stale output publishes governed unavailable
  state instead.
- Governed provider boundary: production resolves a root-owned, bounded
  `/etc/mackesd/navigation-provider.json` that permits only an explicit numeric
  loopback HTTP route and pins an Ed25519 response key. Requests carry a digest
  of the exact route contract; responses must be bounded JSON, match that
  digest/provider/request identity, claim offline attribution, start/end at the
  requested geometry, remain fresh, and verify under the pinned key. Redirect,
  remote, malformed, oversized, timed-out, unsigned, or wrong-key providers
  publish unavailable instead of a route.
- Production path: navigation action → installed worker → provider calculation
  → freshness/contract validation → navigation state publication.
- Focused farm gates on `172.20.0.90`:
  - slot `1`:
  `workers::navigation::tests::provider_result_older_than_request_is_refused`:
  PASS, 1 passed, 0 failed, 4,807 filtered out.
  - slot `2`:
    `workers::navigation::tests::governed_production_provider_is_bounded_and_request_bound`:
    PASS, 1 passed, 0 failed, 4,809 filtered out, including a schema-correct
    local impersonator signed by an untrusted key.
- Remaining epic boundary: package/provision an approved local routing engine,
  its signed-response key and offline dataset, then capture live provider-loss
  and recovery evidence.
