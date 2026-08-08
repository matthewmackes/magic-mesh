# WL-FUNC-021 — provider-loss loopback witness (2026-08-06)

Status: bounded transport/policy evidence only; live provider, daemon, decoder,
and hardware acceptance remain open.

## Command and result

```text
install-helpers/verify-music-network-loss.sh --self-test
verify-music-network-loss: PASS (temporary fixture and trace cleaned on exit)
```

The default bounded run reported:

```json
{"audio_bytes_before_failure":4096,"clean_eof":"clean_eof","fallback_requests":0,"loopback":true,"midstream_failure":"ConnectionResetError","policy":"advance_without_from_zero_replay","primary_bytes_before_failure":4140}
```

The disposable provider binds only to `127.0.0.1`. The clean fixture ends with
FIN; the loss fixture sends the HTTP header plus audio bytes and then performs
an abortive TCP close. The client distinguishes the two outcomes and does not
request `/fallback` after audio has begun. Timeout bounds and cleanup are
enforced by the helper.

## Limitations

This does not prove a live `mde-musicd` request, Airsonic/Jellyfin outage,
mid-track range resume, physical network loss, or audio hardware behavior. The
engine-level 18/18 farm test and the live provider/audio records remain the
authoritative evidence for their narrower boundaries.
