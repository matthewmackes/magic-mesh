# WL-FUNC-018 peer-launch route provenance — 2026-08-13

## Outcome

The reachable peer-app launch boundary now refuses unsafe App VM route
provenance before it can become a typed `SessionRequest::OpenApp` lifecycle
mutation. The serving peer, App VM identity, and client peer must each satisfy
the canonical bounded Workloads identifier grammar. Path-like, control-bearing,
oversized, or otherwise non-canonical route identities therefore fail closed at
the launch-to-session handoff rather than relying on a later session-broker
rejection after publication.

This is distinct from `b121c12b`, which validates the Front Door's discovered
serving-node identity before launch emission. This slice independently protects
the daemon backend boundary and also covers VM/client provenance.

## Changed runtime path

- `crates/mesh/mackesd/src/workers/peer_app_launch.rs`
  - `app_vm_session_request` now admits `node`, `vm_id`, and `client_peer`
    through `WorkloadId` before constructing `OpenApp`.
  - The hostile regression proves malformed values in each of those three
    authority fields produce no lifecycle projection.

## Farm gates

- `.170` / `func018-peer-route-provenance-test-r496b`
  - `cargo test -p mackesd --lib workers::peer_app_launch::tests::guest_launch_rejects_unadmitted_route_provenance_before_session_projection -- --exact --nocapture`
  - Result: **PASS**; 1 passed, 0 failed, 4,944 filtered out.
- `.90` / `func018-peer-route-provenance-clippy-r496`
  - `cargo clippy -p mackesd --lib --features async-services -- -D warnings`
  - Result: **PASS**; strict production-library clippy completed successfully.
- `.196` / `func018-peer-route-provenance-fmt-r496`
  - Synced through `xcp-build.sh`, then ran `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/peer_app_launch.rs` on the farm host.
  - Result: **PASS** after correcting import order.
The initial exact-test attempt on `.50` and an optional full-module attempt on
`.196` were stopped after their final compile/link stages stopped reporting
progress; neither is claimed as evidence. The required exact test was rerouted
to `.170` and completed green. No gate ran on `.130`, and no duplicate test is
claimed.

## Remaining epic acceptance

The slice closes one backend provenance gap. `WL-FUNC-018` still requires the
governed App VM image/release artifacts and post-release installed proof of
catalog search, Workloads start/readiness, Wayland-over-VDI attachment,
disconnect/policy cleanup, reconnect, and absence of host-native Flatpak
execution.
