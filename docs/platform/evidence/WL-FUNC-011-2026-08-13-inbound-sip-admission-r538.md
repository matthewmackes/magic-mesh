# WL-FUNC-011 — inbound SIP identity admission (r538)

Date: 2026-08-13

## Production result

- The concrete SIP provider retains the exact currently ringing provider dialog
  as an untrusted offer instead of silently discarding the inbound event.
- The Collaboration worker normalizes the provider identity and resolves it
  against current signed space membership for both the remote identity and the
  local actor. Absent, malformed, deleted, substituted, or multiply matching
  authority is rejected.
- The worker mints the only opaque `CallId` and authors the only signed
  `CallStarted` fact. The provider can only compare-and-consume the exact offer
  it exposed; replacement and replay fail stale and cannot create a second call
  owner.
- Remote hangup, registration loss, and SIP-agent termination clear an unbound
  offer before it can be admitted later.

## Hostile coverage

- `inbound_sip_identity_admission_is_exact_unique_and_fail_closed` covers
  canonical case normalization, absent/substituted identities, display-name and
  URI-parameter injection, malformed/empty dialog identities, and ambiguous
  membership.
- `inbound_sip_binding_rejects_replaced_and_replayed_dialogs` covers exact
  provider-dialog replacement, one-time binding, and replay rejection.

## Farm gates

- `.170`, slot 1: `cargo test -p mackesd --lib inbound_sip_ -- --nocapture` —
  passed 2/2 after the final source sync.
- `.90`, slot 1: `cargo clippy -p mackesd --all-targets --all-features -- -D warnings`
  — passed against the final source.
- BigBoy `.130`, slot 1: `cargo build -p mackesd --all-targets` — passed before
  the final formatting/visibility and stale-offer clearing delta. The final
  current source was compiled across all targets by the strict Clippy gate; a
  duplicate broad build was intentionally not launched after the progress and
  BigBoy-capacity directives.
- `.90`, slot 1: exact-file `rustfmt --edition 2021 --check` for `collab.rs` and
  `collab_media.rs` — passed against the final source.
- Scoped `git diff --check` for the two production files and this evidence —
  passed.

No live SIP provider, audio, or seat proof was claimed; that acceptance remains
deferred until after the first full release under the active release policy.
