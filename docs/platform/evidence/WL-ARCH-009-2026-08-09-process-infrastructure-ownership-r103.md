# WL-ARCH-009 process-infrastructure ownership — r103

Date: 2026-08-09
Base revision: `3f260a94` (`unify runtime projections and truthful boot status`)
Status: implementation and focused combined-source rerun complete

## Scope

This slice censuses and group-owns three production-reachable infrastructure
paths that previously bypassed the canonical worker registry:

- Control exclusively owns `mesh_service_key_reconciler`, including service
  account/key installation and the 30-second retry task.
- Observation exclusively owns the one-shot `etcd_startup_probe`.
- Each packaged `Type=notify` process owns exactly one of six group-specific
  watchdog registrations (`process_watchdog_control` through
  `process_watchdog_integrations`).

`SpawnBinding::ProcessInfrastructure` distinguishes these starts from supervised
workers and responder threads. `register_process_infrastructure` parses the
exact `serve --group` argument, requires the matching canonical binding and
owner, records the admitted start in the process roster, and fails closed before
filesystem, network, task, or watchdog effects. The service-key retry also
checks daemon shutdown every 250 ms and immediately before each retry effect.

The canonical registry now contains 160 rows. Its complete runtime-field digest
is `a1665a1cfd364133b5adcf9f0b4003913bd5972aa5bca9c827628a05d56dde79`.

## Verification

Farm host/slot:

```text
MCNF_BUILD_HOST=172.20.0.90
MCNF_BUILD_SLOT=arch009-runtime-status-owner-r98
```

Command:

```text
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services worker_role::tests --locked -- --nocapture
```

Completed first result:

```text
26 passed; 2 failed; 0 ignored; 4604 filtered out
```

Both failures were exact expected-value drift caused by this slice, not behavior
failures:

- `canonical_registry_inventory_hash_covers_every_runtime_field` observed the
  new digest above while the test still expected the prior digest.
- `workers_for_rank_is_a_growing_superset` observed 122 Lighthouse rows while
  the test still expected 114; the Workstation expectation likewise advanced
  from 152 to 160.

Those expectations were corrected in the patch. The integrator then synced the
complete release-28 worktree, including the concurrent storage and Android
changes, and reran both focused suites on `.90`:

```text
cargo test -p mackesd --lib worker_role::tests --locked -- --nocapture
28 passed; 0 failed; 4604 filtered out

cargo test -p mackesd --bin mackesd process_group_thread_admission_tests \
  --locked -- --nocapture
6 passed; 0 failed; 56 filtered out
```

This passing rerun includes the exact
`process_infrastructure_is_admitted_only_by_its_registered_group` regression
and the final registry digest/count checks.

Local scoped `git diff --check` passed for `mackesd.rs`, `spawn.rs`, and
`worker_role.rs`.

## Remaining proof

- Prove the eight roster rows and six watchdog owners in an installed grouped
  package.
