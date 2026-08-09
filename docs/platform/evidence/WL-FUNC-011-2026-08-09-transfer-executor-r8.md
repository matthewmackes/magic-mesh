# WL-FUNC-011 S5 clipboard executor authority audit — 2026-08-09

## Outcome

`Clipboard + PublishClipboard` remains fail-closed. No executor was added because
the existing authorities cannot safely turn its contract into a publication.
The registry now names the exact missing provider boundary instead of the broad
"Clipboard Files publication executor" label.

## Production trace

The V2 transfer source contains only an opaque clipboard profile and a
`PayloadRef` (SHA-256, length, and optional content type). It does not contain a
Files object identity, collaboration space, clipboard source/target/session,
signed attribution, sequence, expiry, or destination generation.

The canonical rich clipboard mesh adapter accepts a signed
`ClipboardEnvelopeV2`. For every `FilesReference` offer it requires the exact
`FileRefId` to resolve through a retained `state/collab/file-references/<space>`
projection, match the signed hash and byte count, and match canonical CAS bytes.
Its accepted result is bound to the source peer, target peer, session, and
generation. The existing Files copy executor separately authenticates a
destination-generation command and waits for its projection receipt.

No production registry maps the transfer's opaque clipboard profile and digest
to those identities or receipts. Opening `collab/content/<hash>` directly would
prove byte integrity but would not prove authorization, ownership, target,
session consent, or generation. Treating the Local destination as that missing
authority would also bypass the clipboard envelope and create a second
publication path. Both substitutions were rejected.

## Correction and focused proof

The exhaustive V2 registry still admits only `Local + Copy`. The clipboard row
now reports:

`clipboard profile registry cannot bind the payload to an authorized Files reference, target session, and generation receipt`

The focused scheduler test uses a resolver that panics if called. It proves the
Clipboard job becomes a durable, non-retryable `Unsupported` terminal row with
zero bytes transferred before Files resolution or any publication effect.

Machine 9 (`172.20.0.50`), slot `func011-transfer-r8`:

`cargo test -p mackesd --lib --features async-services v2_clipboard_publish_refuses_unbound_profile_before_files_resolution -- --nocapture`

Result: **1 passed, 0 failed, 4381 filtered out**. Exact-file
`rustfmt --edition 2021 --check` also passed. A package-wide format check was
excluded because it reports pre-existing drift in unrelated files outside this
lane's ownership; it did not report this transfer module.

## Remaining provider work

A real executor requires one production clipboard-profile registry that returns
the authorized Files reference, collaboration space, target node/seat/session,
consent, source generation, and destination generation. The executor must then
publish through the existing signed envelope/Files authorities, observe the
exact generation receipt, and bind cancellation/retry to that same identity.
The authenticated Mesh copy, V2 rsync, sealed SFTP, HTTP resource, browser
scrape, multipart upload, and recurring mirror providers also remain absent.
