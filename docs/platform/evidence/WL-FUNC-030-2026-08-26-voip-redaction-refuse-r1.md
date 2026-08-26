# WL-FUNC-030 source redaction/refuse honesty — voip responder — r1

Date: 2026-08-26
Observed: `2026-08-26T11:59:49Z`–`2026-08-26T12:11:31Z`
Classification: source-unit / farm contract evidence; **not** live Bus
set/get/clear, **not** migrated `gateway.toml`, **not** Activity paint,
**not** `production_admitted`
Source worktree: `agent/drain-worklist-20260725` dirty over `5d2f8e54b`
Control host: `rocky9-kvm2`
`production_admitted: false`

No invented SIP host, username, or password. No `set-gateway` /
`clear-gateway` on a live seat. Construct seats were not occupied.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-030`.
- Prior live GET (Surface, `present:false`, no password):
  `WL-FUNC-030-2026-08-25-surface-gateway-r1.md`.
- Activity already refuses malformed hosts and replayed clears at the UI
  boundary (`validate_gateway_set` / `validate_gateway_clear`). This unit
  matches that refuse on the existing `ipc/voip.rs` responder so a direct
  Bus caller cannot bypass it.

## Source unit

Write scope: `crates/mesh/mackesd/src/ipc/voip.rs` (crate tests in the
same module). Did not touch `activity.rs`, `mackesd.rs`, onboard, or
FUNC-028 transfers.

What landed:

- `get-gateway` still returns `password: ""` plus `password_set`; absent
  readout has no `password` field.
- `GatewayFile` `Debug` redacts a stored secret (`<redacted>`).
- `set-gateway` refuses scheme, path, whitespace, `@`, and embedded-port
  hosts before rewriting `gateway.toml`.
- Empty host still clears a *present* gateway (documented daemon
  shortcut). A second empty-host clear, and `clear-gateway` when the file
  is already absent, refuse with `gateway is already cleared`.
- A replayed armed `clear-gateway` token still refuses `already used`.

Covered tests:

- `malformed_hosts_refuse_before_gateway_io`
- `replayed_clears_refuse`
- `password_never_renders_in_replies_or_debug`
- existing set/get round-trip, redacted resubmit, empty-host first clear,
  unsigned mutation refuse, and exact-body single-use (now including
  replayed armed clear)

## Focused farm evidence

Peer agents held other dirty `mackesd` paths (`bin/mackesd.rs`,
`lifecycle_authority.rs`, `nebula_enroll_client.rs`,
`onboard/remote_push.rs`, `onboard/self_test.rs`). Those were not
reverted. The crate compiled them as dependencies of `-p mackesd`; this
unit filtered to `ipc::voip` so unrelated dirty tests were not the gate.

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=2 \
  install-helpers/xcp-build.sh cargo test -p mackesd ipc::voip --lib -- --nocapture
```

Admission: `.130` slot `2` (`magic-mesh-farm-2`), 81 280 472 KiB free
(required 8 388 608 KiB). Compile `10m 16s`. Result: **9 passed, 0
failed, 0 ignored, 5072 filtered out**; `xcp-build` exit 0.

This is farm implementation/contract evidence, not live Bus, migrated
`gateway.toml`, or installed-seat mutation.

## Leftover

`@leftover:{live-seat}` remains: live Bus set/get/clear plus a migrated
workgroup `gateway.toml` on an acceptance seat. Closing it still needs a
real registrar file (migrated, not invented) and a current-revision seat
whose actions group is up. Do not invent credentials. Do not publish
set-gateway on a live seat from this unit. Do not flip
`production_admitted`.
