# WL-FUNC-020 Cuttlefish relay restart retry — 2026-08-13

## Result

The production Cuttlefish guest transport now tolerates one transient Unix
relay absence during a guest-relay restart. The retry is deliberately confined
to the pre-connect boundary: no governed request bytes have left `mackesd`, so
the retry cannot duplicate an Android launch. Once a connection succeeds, any
authentication, write, read, timeout, or response failure is returned without
an in-process replay because the guest effect may already have occurred.

Changed production scope:

- `crates/mesh/mackesd/src/workers/cloud/verbs/cuttlefish_guest.rs`

The exact regression starts the workload-scoped relay after the first connect
attempt, then proves the single bounded reconnect reaches the authenticated
relay and admits its current, generation-bound guest snapshot.

## Farm gates

- BigBoy `.130`, slot 2: `cargo test -p mackesd relay_restart_before_connect_gets_one_bounded_side_effect_free_retry -- --nocapture` — passed 1/1.
- BigBoy `.130`, slot 3: `cargo clippy -p mackesd --all-targets -- -D warnings` — passed.
- `.196`, slot 1: `cargo build -p mackesd --all-targets` — passed.
- `.50`, slot 2: `cargo fmt --manifest-path crates/mesh/mackesd/Cargo.toml -- --check` — the owned file was clean; the crate-wide command remained red on unrelated pre-existing concurrent formatting drift outside this slice.
- File-only `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/cloud/verbs/cuttlefish_guest.rs` — passed as the permitted tiny local syntax/format probe.
- `git diff --check` over the owned production and evidence files — passed.

## Remaining acceptance

FUNC-020 still requires the signed Cuttlefish image and deterministic guest
packages to be consumed by the first release, remote-session attachment and
guest input to be complete, and the deferred post-release one-node nested-KVM,
app lifecycle, VDI input/audio/reconnect, isolation, upgrade, and live UX
acceptance to run. Those release/live criteria are not claimed by this slice.
