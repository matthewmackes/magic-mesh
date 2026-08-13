# WL-FUNC-021 — mDNS provider generation recovery (r493)

Date: 2026-08-13

## Acceptance gap

The media-source worker recovered from a missing or replaced Bus, but its mDNS
browse generation was opened only once. If that daemon/channel disconnected,
the worker kept every endpoint learned by the dead generation marked reachable
and never rebuilt discovery. A lost Jellyfin-class provider could therefore
remain selectable indefinitely even though the discovery authority that would
expire it no longer existed.

## Implementation

`crates/mesh/mackesd/src/workers/media_sources.rs` now distinguishes an empty
browse receiver from a disconnected generation. On disconnect it:

1. revokes all endpoints learned from that generation before publication;
2. publishes the resulting fail-closed fold while mesh-registry and gateway
   lanes remain available; and
3. starts a fresh mDNS daemon/browser generation without restarting the worker.

The regression shuts down a real `mdns_sd::ServiceDaemon`, observes the closed
receiver, and proves the stale provider is removed before retry state is
published.

## Farm gates

- `172.20.0.196`, slot `func021-mdns-generation-recovery-r493`:
  `cargo test -p mackesd --features async-services --lib workers::media_sources::tests::disconnected_mdns_generation_revokes_stale_provider_before_retry -- --exact --nocapture`
  passed 1/1 (4,933 filtered out).
- `172.20.0.170`, slot `func021-mdns-generation-fmt-r493`:
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/media_sources.rs`
  passed.
- `172.20.0.90`, slot `func021-mdns-generation-clippy-r493b`:
  `cargo clippy -p mackesd --features async-services --lib -- -D warnings`
  passed.

An earlier `.50` clippy invocation was interrupted without a diagnostic when
farm capacity opened elsewhere; the exact strict gate above supersedes it.

## Remaining epic acceptance

This closes one provider-loss/recovery defect. FUNC-021 still requires the
deferred post-release physical renderer, audible provider-loss continuation,
authenticated mutation/rotation, cast, handoff, package, and multi-seat live
proof recorded by the canonical worklist.
