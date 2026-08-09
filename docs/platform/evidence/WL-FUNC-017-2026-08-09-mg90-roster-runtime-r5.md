# WL-FUNC-017 S7 — reachable MG90 roster runtime (r5)

Date: 2026-08-09
Farm: machine 193 (`172.20.0.90`), slot `func017-roster-runtime-r5`
Source: `crates/mesh/mackesd/src/workers/vehicle.rs`
Source SHA-256: `700d95836fe5472f2797f3381021ccc9186237c28288740f4d5f93a01785774c`

## Result

The r4 reachability audit is corrected. `VehicleWorker::run` now owns one `VehicleRuntimeRoster`; configured manager IDs and the governed/discovered MG90 ESN register in that roster, and the existing due status, enrichment, and heartbeat lanes feed it identity-bound snapshots. Approved/enrolled roster routing is the sole production v2 publication authority. The prior direct v2 publisher is test-only.

Local single-gateway behavior uses the same roster. An initial or lost source emits only an explicit `online: false` legacy availability heartbeat. It emits no v2 source claim. Manager loss revokes the row immediately for a local poll failure or after three missed bounded remote intervals; reconnect starts a new `Changed` publication epoch.

Remote rows are read only from the exact configured manager/source topic. `Pending`, `Unknown`, `Revoked`, incomplete enrollment, schema mismatch, source mismatch, and manager mismatch cannot route. A selected remote row suppresses a competing local claim but is never re-published by this worker, preventing feedback loops and remote-manager impersonation.

## Focused verification

- `cargo test -p mackesd --lib --features async-services worker_runtime_roster_ -- --nocapture`: 2 passed, 0 failed.
- `cargo test -p mackesd --lib --features async-services roster_requires_explicit_approval_even_with_complete_manager_set -- --nocapture`: 1 passed, 0 failed.
- `cargo test -p mackesd --lib --features async-services workers::vehicle::tests::roster_ -- --nocapture`: 12 passed, 0 failed.
- Exact-file `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/vehicle.rs`: passed.
- Scoped `git diff --check -- crates/mesh/mackesd/src/workers/vehicle.rs`: passed.

The broader crate formatting check remains noisy from concurrent unrelated dirty files; this slice did not modify or normalize them. No credential value or packet body was logged.

## Live blocker

Machine 193 has no configured `MDE_VEHICLE_GATEWAY` or attached MG90 hardware. A governed multi-manager MG90 seat is still required for physical manager-loss/reconnect and radio/GNSS evidence. No hardware success is inferred from the farm fixtures.
