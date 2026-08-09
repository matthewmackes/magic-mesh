# WL-FUNC-019 authenticated Windows RDP path audit — 2026-08-09

## Outcome

The live credential implementation is not the reason the discovered target
fails to ask for a Windows login. The production Chooser already prompts for an
external endpoint with no sealed credential, keeps the password in a masked and
Debug-redacted buffer, and passes it only in memory to the RDP client.

The discovered resource card cannot safely reach that path yet. It is admitted
as an approval-gated `Desktop` with a capability-bound RDP `Connect` action, but
the active resource UI renders a non-service card as inspection-only. Treating
that catalog card as an operator-authored `Manual` source was rejected during
this audit because it would:

- make publisher-owned identity and endpoint data editable/removable through
  the manual-source preferences and add/remove actions; and
- bypass the existing accepted-receipt resource router.

No production code was changed. The unsafe projection was removed before this
evidence was recorded.

## Production authority path

1. `ChooserState::default` constructs `ResourceBrowserState` with only the
   fixed `resource-publisher-hmac` systemd credential. Without a valid detached
   publisher proof, the catalog remains an explicit read-only compatibility
   view and cannot drive actions.
2. The probed trusted-LAN RDP card declares `AuthStatus::Required`,
   `AuthMethod::LocalApproval`, and `Connect/RequiresApproval` bound to the exact
   RDP transport and native client fingerprints.
3. The daemon resource router supports typed VDI Connect and routes an accepted
   request only to the VDI session authority. It rejects any card action whose
   availability is not `Ready`.
4. The shell resource publisher currently has Workload and Android authority
   request variants, but no VDI request/receipt path and no local-approval
   transition that republishes the same card action as `Ready`.
5. Only after an accepted, fully correlated VDI receipt may the existing
   external-endpoint flow resolve the Windows login. An absent credential raises
   the masked prompt; a bare mesh identity is rejected by live RDP; the secret is
   exposed only when constructing `RdpConfig` in memory.

A card click by itself is therefore not treated as execution of local approval.
The repository already has a stronger receipt router, and bypassing it would
violate the capability/action generation boundary.

## Focused farm verification

Machine 196 (`172.20.0.196`), slot `func019-rdp-auth-r7`:

- resource publisher credential loader exact/bounded/non-following refusal:
  2 passed;
- external endpoint with no sealed credential raises the one-time prompt:
  1 passed;
- live RDP rejects mesh identity without a guest OS login:
  1 passed with `live-vdi`;
- exact VDI Connect routes only through the session authority:
  1 passed in the `mackesd` async-services library;
- stale or unavailable resource actions fail closed:
  1 passed in the same authority suite.

The first accidental `--exact` invocation matched zero tests and is excluded;
the corrected focused invocation above passed one test. No broad suite, live
seat restart, credential mutation, or RDP login attempt was performed.

## Remaining live inputs and implementation blocker

Authenticated render proof for `172.20.146.54:3389` requires all of:

- the approved `resource/publisher-hmac` secret distributed to the seat;
- a typed local-approval transition plus shell VDI invocation/accepted-receipt
  handoff for the exact catalog revision, card, action, transport, and client;
- an operator-supplied Windows username and password through the masked prompt.

The publisher HMAC and Windows password are independent credentials. Neither was
invented, read, persisted, logged, or installed by this audit.
