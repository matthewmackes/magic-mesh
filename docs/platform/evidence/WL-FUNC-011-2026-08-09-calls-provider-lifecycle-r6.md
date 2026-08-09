# WL-FUNC-011 S4 Calls provider lifecycle — 2026-08-09

## Outcome

The reachable Calls path is the existing typed `action/collab/*` command drain,
signed collaboration events, and `state/collab/call-media-*` sidecar. No second
Calls authority or provider was added.

Two production-boundary defects were corrected:

- `StartCall`, `AnswerCall`, DTMF, and mute are refused before event minting
  unless the worker registry contains an adapter admitted for that call kind.
  Decline and hang-up remain available when every provider is gone.
- Unchanged retained readiness is re-probed every worker tick. Provider
  revocation clears stale frame evidence/status, and reconnect can restore
  `live_media_verified` without a synthetic collaboration mutation.

Production constructs an empty registry today. The only provider registration
call site remains the explicit test injection seam, so this change does not
claim a real SIP/WebRTC provider exists.

## Machine 9 live/package audit

Host `172.20.0.50` reported `mcnf-build-home-services`, Fedora 42.

- No installed package matched LiveKit, baresip, Linphone, PJSIP, Janus, Jitsi,
  coturn, Kamailio, FreeSWITCH, or Asterisk.
- No corresponding server/client binary was on `PATH`.
- `pipewire-1.4.11-1.fc42` and `wireplumber-0.5.14-1.fc42` are installed, but
  both user services were inactive in the farm session. These seat graph
  packages are not a SIP/RTP/WebRTC provider.

Exact blocker: the candidate has no installed and production-registered remote
call media adapter/provider. Calls therefore fail admission honestly rather
than publishing fake connected or muted state.

## Focused farm proof

Machine 9, explicit slot `func011-calls-r6`:

- Owned-file `rustfmt --edition 2021 --check`: pass. The crate-wide formatter
  remains blocked by unrelated shared-worktree formatting outside this slice.
- `workers::collab_media::tests::provider_admission*`: 2 passed, 0 failed.
- `workers::collab::tests::empty_media_registry_refuses_fake_connected_call_state`
  with full qualified name and `--exact`: 1 passed, 0 failed.
- `workers::collab_media::tests::unchanged_readiness_is_reprobed_across_revocation_and_reconnect`
  with full qualified name and `--exact`: 1 passed, 0 failed.

The earlier bare-name `--exact` attempt was stopped and is not evidence.

## Source identity

Base revision: `b70e658bf8bc0a0677c934a5d891c31762ebeecd`

- `collab.rs`: `f9eb30458e571cd289e05b3fdd42481c164f91388d770d0852cc85eecb42a788`
- `collab_media.rs`: `f446f73d0cb32a0e90f3d58ed72d03923148f57b0c77b13a548e06f8c6094078`
