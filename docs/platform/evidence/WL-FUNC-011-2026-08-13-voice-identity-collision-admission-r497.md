# WL-FUNC-011 — Voice identity collision admission (r497)

Date: 2026-08-13

## Implemented boundary

`voice_provision` now admits enrolled roster identities before provider or
secret-store effects. Every row in an ambiguous derived-identity group fails
closed when either:

- distinct hostnames normalize to the same Vitelity SIP username; or
- distinct node IDs normalize to the same sealed credential reference.

Rejected rows remain visible as honest `Error` fleet-board states. Unrelated
valid nodes continue provisioning, so one malformed roster group cannot stall
fleet recovery. Rejected identities cannot create a provider sub-account or
receive sealed credentials.

## Farm evidence

- `172.20.0.130`, slot `func011-voice-identity-test-r497`:
  `cargo test -p mackesd --lib workers::voice_provision::tests::reconcile_rejects_colliding_voice_identities_before_provider_effects -- --exact --nocapture`
  passed 1/1 with 4,944 filtered tests.
- `172.20.0.130`, slot `func011-voice-identity-clippy-r497`:
  `cargo clippy -p mackesd --lib -- -D warnings` passed.
- `172.20.0.130`, slot `func011-voice-identity-fmt-r497`:
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/voice_provision.rs`
  passed.

The initial short-name `--exact` test selected zero tests and was rejected as
evidence; the full module-qualified invocation above is the accepted gate.

## Remaining epic acceptance

WL-FUNC-011 still requires remaining provider adapters and session lifecycle
completion, cross-node executors, office fidelity, package/repository gates,
and the deferred post-release live call/collaboration matrix. This slice proves
only the reachable provisioning identity/provenance boundary.
