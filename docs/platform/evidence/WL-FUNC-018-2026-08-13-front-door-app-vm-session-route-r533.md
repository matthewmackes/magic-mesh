# WL-FUNC-018 — Front Door App VM session route (r533)

Date: 2026-08-13

## Production slice

`peer_app_launch` now executes the existing `guest-app-vm` Front Door contract
instead of sending every request through the legacy host `.desktop` argv path.
After validating the explicit App VM/session identities and consuming the exact
incoming launch capability, the root daemon:

- builds the shared typed `SessionRequest::OpenApp`;
- signs that exact request for the closed `vdi-session-open` authority context;
- durably claims the effect before publishing it to `action/vdi/session`; and
- records a typed success, refusal, or indeterminate launch result.

Missing signing credentials, incomplete lifecycle identity, unsupported guest
capabilities, Bus replacement, and publish failure all fail closed. A guest
Flatpak never falls through to host process execution, and restart recovery does
not repeat an ambiguously claimed App VM dispatch.

Owned production file:

- `crates/mesh/mackesd/src/workers/peer_app_launch.rs`

## Farm gates

- BigBoy `172.20.0.130`, slot `func018-appvm-route-test-r1`:
  `cargo test -p mackesd --features async-services
  guest_launch_dispatches_one_authorized_open_app_to_session_broker --
  --nocapture` — passed 1/1 (4,970 unrelated unit tests filtered out).
- `172.20.0.90`, slot `func018-appvm-route-clippy-r1`:
  `cargo clippy -p mackesd --features async-services --all-targets -- -D
  warnings` — passed.
- `172.20.0.196`, slot `func018-appvm-route-fmt-r3`:
  `rustfmt --edition 2021 --check
  crates/mesh/mackesd/src/workers/peer_app_launch.rs` — passed.
- Local scope-only `git diff --check` — passed before evidence recording.

## Remaining WL-FUNC-018 acceptance

- Produce and bind the current governed App VM image/profile hash and approved
  Flatpak runtime supply in the first full release.
- Complete Front Door permission/progress/focus/failure UX and its shared-style
  render/model coverage without host backend I/O.
- After the first full release, perform the deferred non-blocking one-node live
  acceptance for Wayland presentation, input/audio, persistence, stop/crash,
  reconnect/cleanup, sandbox/SELinux, package upgrade, and host-secret denial.
