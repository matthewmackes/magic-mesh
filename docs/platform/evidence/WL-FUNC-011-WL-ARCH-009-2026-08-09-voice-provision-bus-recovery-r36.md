# WL-FUNC-011 / WL-ARCH-009 — voice-provision Bus recovery (r36)

Date: 2026-08-09

Production source: `crates/mesh/mackesd/src/workers/voice_provision.rs`

Source SHA-256:
`a36d2485a3f5e7b29a1b4e40eaeaecd746c2d9f6c15ef65ed2bd46ccd0c80a18`

## Correction and state semantics

`VoiceProvisionWorker` no longer becomes permanently idle when its Bus root is
unresolved or unopenable. Explicit roots remain exact; otherwise normal
mde-bus resolution falls back to `mde_bus::SYSTEM_BUS_ROOT`. The same worker
retries unresolved roots, failed opens, and failed activation reads with
shutdown-aware exponential backoff bounded from 10 ms to 2 s.

The four raw `action/voice/*` lanes (`provision`, `did-route`, `shared-config`,
and `failover`) are privileged transient mutations. Startup reads all four
tails into a candidate cursor map and installs it only after every tail read
succeeds. Retained provider/config effects therefore never replay. Messages
published after that activation snapshot drain from the installed tail,
including the first forward message on a lane that was empty at activation.

The token-free `.mackesd-voice-authorized-intents.json` projection is durable
desired state. It is restored before Bus recovery begins and is folded with
newly authorized DID-route, failover, and shared-config intents; it is not
discarded or re-authorized from retained armed requests.

Every runtime action lane is read into one candidate sweep before any cursor is
advanced, capability is consumed, durable journal is changed, provider action
runs, or state is published. A later-lane Bus read failure discards the entire
candidate sweep and defers reconciliation, so unavailable state cannot
masquerade as an empty command lane.

## Focused farm proof

Host: XEN-BIGBOY (`172.20.0.130`)

Slot: `voice-provision-bus-r36`

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=voice-provision-bus-r36 \
  ./install-helpers/xcp-build.sh cargo test -q -p mackesd \
  --features async-services --lib \
  workers::voice_provision::tests::late_bus_recovers_without_replay_and_failed_reads_defer_full_sweep \
  -- --exact --nocapture
```

Final post-format result: `1 passed; 0 failed; 4,465 filtered out`. The same
worker survived an unresolved root, an unopenable root, and a failed atomic
tail activation; skipped a retained provision request; preserved an existing
durable DID intent; staged a new forward DID intent before a later-lane read
failure without consuming or persisting it; then folded that first forward
message into the existing durable projection after reads recovered.

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=voice-provision-bus-r36 \
  ./install-helpers/xcp-build.sh cargo test -q -p mackesd \
  --features async-services --lib \
  workers::voice_provision::tests::service_bus_root_falls_back_to_the_shared_system_spool \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,465 filtered out`.

The final source passed farm `rustfmt --edition 2021 --check`, a farm-scoped
`git diff --no-index --check`, and local scoped `git diff --check`. Existing
crate warnings were unrelated to this slice. No broad suite, package build,
live Vitelity call, or filler test was run.

## Blockers

None for this focused startup/recovery correction. Live Vitelity credentials
and provider-side provisioning were intentionally outside this deterministic
Bus-recovery proof.
