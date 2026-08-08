# WL-ARCH-009 — canonical spawn registry foundation (2026-08-05)

The daemon inventory now contains one canonical registration for every literal
production worker name found in `mackesd.rs` and `spawn.rs`. The 145 rows are
classified in the registration itself as 78 `spawn_tiered` starts, one
runtime-named supervisor start, 46 direct supervisor starts, and 20 bounded
responder/maintenance threads. The former test-only `NON_TIERED_WORKERS` and
`DYNAMIC_SPAWNS` exception lists are gone.

Each newly registered row receives one of the six governed groups plus the
existing bounded cadence, queue, cache, resource, ownership, cleanup, and
restart-policy contract. `workers_for_class` and the neutral
`worker_contracts` projection now expose the complete possible roster rather
than hiding directly bound workers.

The bidirectional drift guard reads the production start sites and rejects:

- a tiered registration without a `spawn_tiered` site;
- a `spawn_tiered` site without a tiered registration;
- a literal direct/responder start without a registration;
- a direct/responder registration without a literal start;
- a tiered or runtime-named worker pushed through the direct path; and
- direct-supervisor restart-policy drift or a responder acquiring a supervisor
  restart policy.

## Verification

On farm node `172.20.0.50`, slot `arch009-registry`:

```text
cargo test -p mackesd --lib \
  worker_role::tests::worker_spawns_and_the_census_do_not_drift -- --nocapture

1 passed; 0 failed; 0 ignored; 4481 filtered out
```

This is a process-isolation foundation, not completion of WL-ARCH-009. Typed
dependencies, publications, subscriptions, actions, and entity/output kinds
remain incomplete, and the six independently supervised process entrypoints do
not exist yet.
