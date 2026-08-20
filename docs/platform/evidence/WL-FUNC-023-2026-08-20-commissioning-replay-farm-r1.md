# WL-FUNC-023 commissioning replay boundary evidence — 2026-08-20

## Scope

This slice adds `onboard::invite::redeem_once`, the typed commissioning
boundary for a real enrollment handoff. The existing `redeem` projection remains
non-consuming for the endpoint-less wizard preview; `redeem_once` validates
version/mesh/expiry/ledger scope and then atomically consumes the canonical
invite payload before returning the `JoinToken`. A replay, alternate encoding,
or concurrent loser receives `RedeemError::NotIssued`. Invalid scope is
refused before consumption, preserving corrected-forward retry for the valid
operator input.

The focused tests cover first-winner consumption, replay refusal through the QR
twin, and foreign-mesh refusal without burning the valid bearer. This slice
changed no excluded transport, CLI, lifecycle-controller, release, desktop,
governance, worklist, or media files; unrelated dirty files were preserved.

## Source identity

- Base revision at dispatch: `c64656261e5affa349722c5e44f9c2dacc7528ce`
- Working-tree patch digest (`invite.rs` diff): `d867c834df7d33c8f1f4563b57359b4db4210d0ccde21dcf0a208a179baae716`
- Formatting: `cargo fmt -p mackesd -- --check` — passed locally (format-only
  exception permitted by the farm governance).

## Farm verification

The required focused command was admitted and dispatched with the dirty
working-tree patch:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=2 \
  install-helpers/xcp-build.sh cargo test -p mackesd \
  onboard::invite -- --nocapture
```

- Host: `172.20.0.90`, slot `2`
- Admission: `19,563,248 KiB` free; required `8,388,608 KiB`
- Result: build reached `mackesd` test linking, then failed with
  `rustc-LLVM ERROR: IO failure on output stream: No space left on device`
- Exit: `101`

The governed retry used the same command and source identity with
`CARGO_INCREMENTAL=0`:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=3 CARGO_INCREMENTAL=0 \
  install-helpers/xcp-build.sh cargo test -p mackesd \
  onboard::invite -- --nocapture
```

- BigBoy admission refused before sync: `2,571,616 KiB` free, below the
  `8,388,608 KiB` sync requirement.

A second admitted retry on `172.20.0.50`, slot `2`, with incremental
compilation disabled reached linking but failed because `mold` reported the
disk full (`cc` exit `1`, cargo exit `101`). A lean `--no-default-features`
retry was not admitted because that node had only `213,328 KiB` free.

## Blockers and non-claims

The patch's focused farm tests could not complete because available farm
storage was exhausted; this is a capacity blocker, not a reported Rust test
failure. The farm output also retained the pre-existing unused-method warning
in `workers/call_media.rs`, which is outside this ownership scope and was not
changed. This evidence does not claim live Bus delivery, live SSH enrollment,
provider effects, installed first boot, or physical-seat acceptance.
