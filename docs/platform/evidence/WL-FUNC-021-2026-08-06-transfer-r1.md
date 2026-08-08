# WL-FUNC-021 — typed Music playback handoff admission (2026-08-06)

## Implemented slice

The versioned Music workspace action contract now carries an explicitly bounded
`target_peer` for the already-admitted `transfer` action. The daemon applies
that action through the existing authenticated, replay-safe workspace Bus lane
and posts the existing durable peer handoff intent; it does not add a GUI-owned
transport, queue, or peer-state writer.

The action fails closed when the target is missing, malformed, the local peer,
the local engine is absent or inactive, or the retained playback owner is a
different peer. Authorization uses the distinct `peer-takeover` scope rather
than broadening the normal workspace token. The existing owner-yield and
target-resume serve-loop paths remain the only actors that pause, persist, and
resume playback.

Changed files:

```text
crates/services/mde-musicd/src/domain.rs
crates/services/mde-musicd/src/bus_responder.rs
```

## Farm verification

All compile/test/format work used explicit farm hosts and isolated slots:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-transfer-full-fallback-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 166 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-transfer-focused-r3 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd \
  typed_workspace_queue_actions_use_the_shared_queue_authority -- --nocapture
result: 1 passed, 0 failed; 165 filtered out

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-transfer-format-r3 \
  install-helpers/xcp-build.sh cargo fmt --check -p mde-musicd
result: pass
```

The required BigBoy `.130` full-suite attempt used
`MCNF_BUILD_SLOT=music-transfer-full-r1` and returned `No route to host`
during synchronization. No BigBoy result is claimed. Reachable farm scratch
workspaces were removed after completion. Local `git diff --check` passed.

## Remaining proof

This fixture-backed handoff result does not prove an authenticated UI action
token issuer, a live two-seat target, audible provider decode, DLNA target
handoff, network-loss playback, Dell/seat-15 acceptance, or release
promotion. Those remain named production gates in the canonical Worklist.

## Source hashes

```text
d5914daabdcc5f58be848d1b769d2122ac4705a14dcaae0545621c3094fe0fc0  crates/services/mde-musicd/src/domain.rs
cc5ff6d7eda9eaa7cc37d5ec0559bd15ecb6d62ec63bcf4548e674081baffcdc  crates/services/mde-musicd/src/bus_responder.rs
```

Working-tree base revision: `e52322ec` (changes are intentionally
uncommitted).
