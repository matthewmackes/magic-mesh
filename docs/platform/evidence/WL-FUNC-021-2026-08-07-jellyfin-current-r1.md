# WL-FUNC-021 current Jellyfin gate (2026-08-07)

The current dirty tree passed the farm-routed Jellyfin package gate on build
host `.90` with slot `music-jellyfin-current-r1`:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-jellyfin-current-r1
./install-helpers/xcp-build.sh cargo test -p mde-jellyfin --locked -- --nocapture
```

Result:

```text
90 unit tests passed
12 browse integration tests passed
2 outage integration tests passed
9 playback integration tests passed
1 doctest passed
114 passed, 0 failed
```

This advances the typed Jellyfin/library/cache boundary. It is farm evidence
only; live provider-loss recovery, physical renderer, cross-seat handoff, and
five-seat CPU/NWS proof remain separate acceptance gates.
