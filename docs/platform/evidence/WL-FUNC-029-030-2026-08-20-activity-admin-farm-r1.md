# WL-FUNC-029 / WL-FUNC-030 Activity admin evidence — 2026-08-20

## Implementation slice

Source base: `b1847d196c4882933320b33527f99e9a7527b095` with the scoped,
uncommitted change in
`crates/desktop/mde-collab-egui/src/activity.rs`.

Communications Activity already carries the shared fleet-voice and SIP-gateway
admin surface over retained projections and typed sinks. This slice completes
the gateway contract at that same surface boundary:

- exposes the responder's optional REGISTER expiry in the bounded form;
- refuses zero or out-of-range expiry before publishing `set-gateway`;
- clears the locally retained gateway form, including the password field, after
  an admitted `clear-gateway`;
- retains the voice-admin empty-state, projection rendering, typed
  provision/DID-route/failover/shared-config/cutover verbs, gateway set/get/clear
  verbs, and password-redacted readout behavior.

No mackesd worker or responder contract was changed. No release, governance, or
worklist file was changed.

## Focused farm verification

Farm admission was checked with `./install-helpers/farm-topology.sh table`:
five of five nodes were up and ten of ten heavy slots were free. The focused
gate ran on `.50`, slot `0`, through the governed farm helper:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=0 \
  install-helpers/xcp-build.sh cargo test -p mde-collab-egui activity_ --lib -- --nocapture
```

Result: `11 passed, 0 failed, 151 filtered out`.

The gate covers Activity rendering, honest empty voice/gateway states, retained
voice and gateway projections, typed verb ordering, password redaction, and the
new gateway expiry refusal cases. It is farm implementation/contract evidence,
not live Bus, provider, migrated `gateway.toml`, or installed-seat evidence.

## Remaining boundary

The worklist's live round-trip and unchanged migrated `gateway.toml` acceptance
still require a governed live Bus/provider path. No live mutation was attempted
in this implementation turn.
