# WL-ARCH-010 / WL-ARCH-009 — Cloud Bus transaction recovery r68

Date: 2026-08-09

## Scope and semantics

- Production Cloud Bus transactions resolve an explicit override first, then the current shared user resolver, then canonical `mde_bus::SYSTEM_BUS_ROOT`. Every activation, action sweep, reply, reachability read, and state publication uses a fresh `Persist` open. The existing explicit `with_bus_root(None)` test/offline disable remains intact.
- Activation is keyed to the current SQLite index device/inode. Topic enumeration and every existing `action/cloud/*` tail read stage into a replacement cursor map; any failure preserves the prior activation. Retained actions are skipped, while the first request on a dynamically appearing action topic is forward work.
- Each runtime sweep stages every action topic/message, required reachability mirror, and existing mutation transaction record before backend dispatch. List/read/decode failure defers the whole staged pass without backend effects or cursor changes.
- Missing-body, malformed-schema, and placement-gated replies are required writes. Their cursors advance only after reply publication succeeds. Reachability open/read/decode errors defer instead of becoming false-unreachable gates.
- Mutations write a durable per-request `Claimed` record before dispatch, a `Completed` record containing the typed reply after dispatch, and `Delivered` only after the reply exists. Reply failure or daemon restart recovers the outbox without repeating the backend effect. A recovered `Claimed` record with no completed outcome emits an honest indeterminate gate and is never re-executed.
- Cloud state publication returns `Result`; dirty state and heartbeat timing commit only after publication. Late and replaced Bus indexes trigger same-worker reactivation and corrected-forward state publication. A focused injected first state-write failure proved retry rather than false success.

## Farm verification

Farm host: machine193, `172.20.0.90`

Slot: `cloud-bus-r68`

Source SHA-256: `5b8e8d5630035ab5760041751c2a830e79272ce536ad2ae9c170361dedcc7963`

The detached verification worktree contained the repository baseline plus only `crates/mesh/mackesd/src/workers/cloud/mod.rs` from this slice.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=cloud-bus-r68 \
  /tmp/magic-mesh-cloud-r68/install-helpers/xcp-build.sh cargo test \
  -p mackesd --features async-services --lib \
  workers::cloud::tests::reply_failure_recovers_durable_mutation_without_repeating_effect -- --exact

Result: PASS — 1 passed, 0 failed, 4552 filtered out. The backend mutation ran
once; the injected reply failure left its cursor uncommitted and its completed
reply durable. A newly constructed worker recovered and delivered one reply
without another backend call. The same test proves malformed reply failure
retains its cursor and retries to one reply.
```

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=cloud-bus-r68 \
  /tmp/magic-mesh-cloud-r68/install-helpers/xcp-build.sh shell

Remote exact commands:
cargo test -p mackesd --features async-services --lib \
  workers::cloud::tests::run_recovers_late_and_replaced_bus_without_replaying_retained_actions -- --exact
cargo test -p mackesd --features async-services --lib \
  workers::cloud::tests::activation_tail_prime_is_atomic_and_dynamic_first_action_executes_once -- --exact
cargo test -p mackesd --features async-services --lib \
  workers::cloud::tests::final_lane_and_reachability_read_failures_defer_all_backend_effects -- --exact

Results: PASS — each command ran 1 exact test with 0 failures and 4552 filtered out.
- One running worker recovered from an initially unopenable Bus and a later same-path index replacement. Both retained backlogs were skipped, while each replacement index's first dynamic forward action received one reply. An injected initial state-publication failure corrected forward.
- Injected final-lane tail failure left the previous cursor set untouched. Successful activation skipped both retained lanes; a newly appearing topic's first request and its next request each executed exactly once.
- Injected final-lane message read failure and malformed required reachability state each deferred the complete pass with no cursor change and no backend call.
```

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=cloud-bus-r68 \
  /tmp/magic-mesh-cloud-r68/install-helpers/xcp-build.sh cargo test \
  -p mackesd --features async-services --lib \
  workers::cloud::tests::default_bus_root_uses_the_shared_mde_bus_resolver -- --exact

Result: PASS — 1 passed, 0 failed, 4552 filtered out. The pure fallback assertion
selects canonical `SYSTEM_BUS_ROOT` when the ordinary resolver is absent.
```

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=cloud-bus-r68 \
  /tmp/magic-mesh-cloud-r68/install-helpers/xcp-build.sh shell

Remote commands:
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/cloud/mod.rs
sha256sum crates/mesh/mackesd/src/workers/cloud/mod.rs

Result: PASS; rustfmt produced no diff and sha256sum returned the source hash above.
Local scoped `git diff --check` also passed.
```

The crate emitted its existing warning set during test compilation; no warning was introduced as a test failure or suppressed for this evidence.

## Residual caveats

The Bus transaction record provides durable at-most-once mutation dispatch and reply recovery, not a cross-resource atomic commit with an external backend. A process crash after `Claimed` but before durable `Completed` cannot prove whether the external effect occurred; recovery deliberately does not repeat it and returns an honest indeterminate gate. This avoids duplicate destructive effects but may require operator reconciliation when the effect never began. A wholesale Bus-index replacement also removes records held only in that index; activation still tail-primes retained action rows in the replacement so they do not replay, while a newly published action with a new ULID is correctly treated as new forward work.
