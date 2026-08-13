# WL-FUNC-011 clipboard URI attribution (r508)

Date: 2026-08-13

## Production result

The native Collaboration clipboard now labels content as `Uri` only when the
complete clipboard value is a bounded HTTP(S) URI with a non-empty authority.
Text that merely begins with a scheme, contains prose or control/whitespace, or
has a missing/malformed authority remains `Text`. The exact admitted clipboard
bytes, content hash, source attribution, and existing size bound are unchanged.

This closes a typed-attribution gap where malformed or mixed clipboard text
could previously be presented as a shared URI based only on its prefix.

## Farm evidence

BigBoy `172.20.0.130` was explicitly excluded and was not contacted.

- `.50`, slot `func011-uri-test`: `cargo test -p mde-collab-egui
  uri_kind_requires_one_complete_bounded_http_uri -- --nocapture` passed 1/1
  (134 filtered).
- `.90`, slot `func011-uri-clippy`: `cargo clippy -p mde-collab-egui --lib --
  -D warnings` passed.
- `.170`, slot `func011-uri-fmt`: `cargo fmt -p mde-collab-egui -- --check`
  passed.
- `git diff --check` passed.

## Remaining epic acceptance

WL-FUNC-011 still requires first-release integration and the deferred
post-release installed collaboration proof, including provider-backed Calls,
cross-node transfer executors, native office transport, migration/hard-cut, and
offline attribution/replay behavior. This checkpoint does not claim those
broader outcomes.
