# WL-FUNC-016 VDI clipboard metadata admission — 2026-08-09

## Gap closed

`clipboard_bridge` decoded attacker-controlled action JSON before the shared
privileged-action body ceiling ran, while `session_id` and optional `source`
were unrestricted strings. A small clipboard value could therefore carry
unbounded routing/attribution metadata into allocation and capability-target
construction.

## Implementation

- Reject action bodies over 64 KiB before JSON decoding, matching the existing
  `ActionAuthorizer` ceiling.
- Require nonempty, unpadded, safe `session_id`, `target_seat`, and `source`
  metadata, each bounded to 128 bytes.
- Hostile tests cover pre-decode oversize refusal, overlong/path-shaped metadata,
  and a correctly signed hostile action producing no clipboard write or fold.

Production source SHA-256:
`f9772c332e017372bc9656eec7f1ec37536c562626ac19c62f5b7c71c46786ea`.

## Farm verification

Host `172.20.0.50`, slot `func016-clipboard-metadata-r6-20260809`:

- `cargo test -p mackesd --lib clipboard_bridge::tests -- --nocapture`:
  **31 passed, 0 failed**.
- Exact-file `rustfmt --edition 2021 --check`: passed in the integrated tree.
- Changed-file `git diff --check`: passed.

The repository-wide formatter remains blocked by unrelated pre-existing drift;
the exact production file is clean. No live DRM or guest claim is made by this
bounded admission checkpoint.
