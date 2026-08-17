# WL-FUNC-023 remote-push farm baseline — 2026-08-16

This is a focused implementation baseline for the shared ONBOARD remote-push
executor. It is not live-seat or live-peer acceptance.

## Previous command

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=wl-rel007-remote-push-1 \
  install-helpers/xcp-build.sh cargo test -p mackesd remote_push --locked -- --nocapture
```

## Previous result

- Farm host: Bigboy `172.20.0.130`.
- Farm slot: `wl-rel007-remote-push-1`.
- Source synchronized from the clean worktree at the start of the job.
- Build/test profile completed in 4m47s.
- `27 passed, 0 failed`; 4,994 filtered from the library target.
- The binary and integration targets completed with zero selected tests.

The passing cases cover typed action ordering and refusal, redaction,
bootstrap/day-2 target separation, signed-bundle freshness and signer checks,
nonce replay handling, thin-lighthouse policy, local application, and the
injected Bus/SSH transport seams.

## Boundary

`SshBootstrap` still returns the intentional typed `NotWired` result when the
`async-services` feature is disabled or `MACKESD_BOOTSTRAP_SSH_KEY` is absent.
The suite does not claim a fresh-box enrollment or live peer Bus
acknowledgement.

## Follow-up verification — 2026-08-17

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mackesd onboard::remote_push \
  --features async-services -- --nocapture
```

- BigBoy `172.20.0.130`, slot `1`.
- Build/test completed successfully in 5m21s.
- `26 passed, 0 failed`; 4,997 filtered from the library target.
- The follow-up includes bounded SSH argv construction and host/bearer
  injection refusal. It still does not claim live fresh-box enrollment.
