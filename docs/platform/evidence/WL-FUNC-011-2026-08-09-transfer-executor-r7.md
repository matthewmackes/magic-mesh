# WL-FUNC-011 S5 transfer executor admission — 2026-08-09

## Production correction

The production V2 scheduler previously sent both `Local + Copy` and
`Mesh + Copy` through the same Files content-cache executor. A Mesh source that
was already materialized locally could therefore complete after a local
generation commit without an authenticated transport session or an
acknowledgement from the addressed remote node.

The scheduler now consults one exhaustive kind/operation registry before it
resolves either endpoint. Only `Local + Copy` reaches the existing CAS-backed
Files executor. `Mesh + Copy` reaches a durable, non-retryable `Unsupported`
terminal row naming the absent authenticated mesh transport and remote
acknowledgement provider; it performs no resolver read, byte copy, or
destination commit. Existing cancellation/retry state remains daemon-owned, and
the reachable Local executor retains its authorized destination-generation
publication and canonical projection confirmation.

## Registry audit

The strict contract admits eleven kind/operation rows across nine families:

- Ready: Local/Copy — Files CAS copy plus authorized generation projection.
- Blocked: Mesh/Copy — authenticated mesh transport and remote acknowledgement.
- Blocked: Rsync/Sync — V2 rsync profile executor.
- Blocked: SFTP/Copy, SFTP/Download, SFTP/Upload — sealed SFTP executor.
- Blocked: HTTP/Download — HTTP resource executor.
- Blocked: Scrape/Scrape — browser scrape materialization provider.
- Blocked: Multipart/Upload — sealed multipart upload executor.
- Blocked: Recurring/Mirror — recurring mirror scheduler and executor.
- Blocked: Clipboard/PublishClipboard — Clipboard Files publication executor.

This is not evidence for seven working executors. Production has one reachable
V2 executor path; the remaining provider classes are explicitly unavailable.

## Focused farm verification

Machine 9 (`172.20.0.50`), slot `func011-transfer-r7`:

`cargo test -p mackesd --lib --features async-services v2_executor_registry_ -- --nocapture`

Result: **2 passed, 0 failed, 4378 filtered out**. The tests prove registry-row
coverage/uniqueness and prove Mesh refusal occurs before the resolver can read a
locally materialized cache entry. Exact-file `rustfmt --edition 2021 --check`
also passed. The crate-wide format check remains blocked by unrelated existing
format drift outside the owned transfer path and is not counted as a gate.

## Remaining live acceptance

Cross-node execution remains blocked until an authenticated mesh transport can
bind source generation, cancellation, retry, and remote acknowledgement to the
same transfer identity. The named protocol/provider blockers above also remain.
A live Local commit still requires the production publisher credential and
Collaboration projection authority; this change does not invent either.
