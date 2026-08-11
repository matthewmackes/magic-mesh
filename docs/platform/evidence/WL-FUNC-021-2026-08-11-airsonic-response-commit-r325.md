# WL-FUNC-021 Airsonic response commitment — 2026-08-11

- Scope: proxy failures are classified by whether an HTTP response has been
  committed to the client.
- Hostile boundary: a truncated upstream body after a `200` response closes the
  stream without appending a second `502` response.
- Focused gate: `cargo test -p mackesd --features async-services --lib workers::media_airsonic_proxy::tests::truncated_provider_response_cannot_append_a_second_http_reply -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1, admitted with 9,295,020 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,847 filtered out.
- Remaining boundary: live provider truncation and installed gateway proof remain.
