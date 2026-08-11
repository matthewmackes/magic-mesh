# WL-FUNC-011 destination generation acknowledgement — 2026-08-11

- Scope: the typed Local/Mesh Copy executor no longer treats changed
  destination bytes as completion. After the signed `CommitFileGeneration`
  path returns, it re-resolves the canonical Files identity, verifies current
  metadata twice, and requires either a monotonic generation advance carrying
  the exact copied digest/size or an exact already-converged replay.
- Hostile boundary: a resolver that changes destination bytes without
  publishing the canonical generation returns `PublicationUnconfirmed`; retry
  after a lost acknowledgement succeeds idempotently from the advanced exact
  generation. Resolvers without mutation authority default to explicit
  `MutationUnsupported`.
- Farm: `172.20.0.90`, slot `1`.
- Focused gate: `install-helpers/xcp-build.sh cargo test -p mackesd --features
  async-services
  workers::transfers::v2::tests::destination_generation_acknowledgement_blocks_false_completion_and_allows_replay
  -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed, 4,810 filtered out.
- Isolation note: an initial compile encountered unrelated dirty Cloud code.
  The passing run used the same disposable slot with only that unrelated Cloud
  snapshot restored to `HEAD`; the transfer patch and its dependencies were
  unchanged.
- Remaining proof: live cross-node acknowledgement and the other typed executor
  families; seven-executor parity is not claimed.
