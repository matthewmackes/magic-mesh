# WL-FUNC-019 — session-roaming recovery provenance (r494)

Date: 2026-08-13

## Executable gap

The live session-open path rejects empty, oversized, control-bearing, and
path-like session/resource route identities. The session-roaming worker restores
its observed roster directly from the replicated `SessionStore`, however, and
previously checked only duplicate session IDs. A unique substituted recovery row
could therefore carry an unsafe `id`, `serving_peer`, `vm_id`, or `client_peer`
into reconnect/release planning and subsequent shared-plane mutation.

## Implemented behavior

`crates/mesh/mackesd/src/workers/session_roaming.rs` now re-attests every
recovered route component against the live-open identity grammar before admitting
the roster. Any malformed identity fails the complete convergence tick closed,
preserving the existing shared plane rather than reconnecting, releasing, or
republishing an untrusted resource route. Duplicate-session recovery remains
fail-closed through a typed admission error.

The focused regression injects a path-bearing substituted serving-peer identity
into a recovered disconnected session and proves roster admission rejects it
before roaming planning.

## Farm evidence

- `172.20.0.90`, slot `func019-roaming-provenance-test-r494b`:
  `cargo test -p mackesd workers::session_roaming::tests::restarted_roaming_rejects_untrusted_resource_route_provenance -- --exact --nocapture`
  passed 1/1 with 4,937 library tests filtered.
- `172.20.0.50`, slot `func019-roaming-provenance-clippy-r494`:
  `cargo clippy -p mackesd --lib -- -D warnings` passed.
- `172.20.0.170`, slot `func019-roaming-provenance-fmt-r494b`:
  file-scoped `rustfmt --edition 2021 --check` passed for
  `crates/mesh/mackesd/src/workers/session_roaming.rs`.

An initial file-format invocation found one multiline `write!` formatting diff;
the source was corrected and the r494b file-scoped gate passed. An initial
BigBoy test command used an incomplete exact test name and was terminated before
it could produce a misleading zero-test result; r494b is the authoritative
focused result above.

## Remaining epic acceptance

This closes the recovered-roster identity-admission gap. WL-FUNC-019 still needs
the worklist's deferred post-release installed route/capture and live recovery
matrix proving universal resource discovery, stable deduplication, typed actions,
and safe local/remote reconnect behavior across selected seats and peers.
