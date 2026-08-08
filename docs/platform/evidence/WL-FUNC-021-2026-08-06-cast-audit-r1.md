# WL-FUNC-021 cast transport-response audit — 2026-08-06

Status: bounded S5 implementation evidence; live renderer, Chromecast,
mesh-owner, and seat-handoff acceptance remain open.

## Finding and fix

The existing cast boundary already rejected malformed/non-finite/negative or
overlong seek positions, bounded discovery and request metadata, and rejected
non-2xx DLNA action responses. One fail-closed gap remained: a renderer could
send a successful status with an incomplete response and the caster would
advance the DLNA sequence or report success based on the status line alone.

`crates/desktop/mde-media-core/src/cast.rs` now validates the response framing
before accepting any 2xx action. Missing header termination, malformed or
conflicting `Content-Length`, and a body shorter than the declared length are
reported as `CastError::Rejected("… incomplete HTTP response")`. The loopback
regression sends a truncated `200` for `SetAVTransportURI`; the caster refuses
the action and never proceeds to `Play` or returns `CastOutcome`.

## Hostile coverage

- `expect_2xx_rejects_a_truncated_success_response` covers a short body against
  a declared `Content-Length`.
- `dlna_truncated_success_response_never_claims_cast_success` covers the real
  TCP DLNA path and proves that the failed action stops the ordered cast.

## Verification

BigBoy (`172.20.0.130`), slot `media-cast-response-fix-20260806-r1`:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=media-cast-response-fix-20260806-r1 \
install-helpers/xcp-build.sh cargo test -p mde-media-core cast -- --nocapture

test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 229 filtered out
```

`git diff --check` passed. The crate-level BigBoy `cargo fmt --check` lane
was blocked by pre-existing formatting differences in the out-of-scope
`crates/desktop/mde-media-core/src/roaming.rs`; that file was not changed.
The local standalone `rustfmt` probe was unavailable because `rustfmt` is not
installed on the development host.

Source SHA-256:

```text
7fc7172e3ac83d8d4ef5de7298667240649183418405d85963acdcbe8cc0fa60  crates/desktop/mde-media-core/src/cast.rs
```

No live renderer, Chromecast, mesh peer, or seat mutation was attempted.
