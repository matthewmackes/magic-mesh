# WL-FUNC-021 — live provider-loss continuity, release 11 (2026-08-08)

## Scope

This was a controlled seat-local outage on the authorized non-production seat
15. It did not stop or modify the shared Airsonic server and did not affect any
other seat. A temporary nftables output rule rejected only seat 15 traffic to
the configured provider endpoint `172.20.0.2:4040`; an exit trap removed the
entire uniquely named probe table after twelve seconds.

## Result

The existing bounded observer sampled release-11 `mde-musicd` every two
seconds. Samples 1-6 were healthy. Samples 7-10 reported the provider
unavailable while the Music service remained active and typed cached catalog
and playback-state requests continued to succeed. Sample 11 returned healthy
after the rule was removed.

The observer passed the complete healthy → provider loss → healthy transition.
Post-proof checks confirmed the nftables probe table was absent, Airsonic ping
was healthy, and `mde-musicd.service` remained active with `NRestarts=0`.

The helper's final message says “natural provider recovery” because it is an
observation-only classifier; this evidence does not repeat that characterization.
The transition was deliberately induced at the network boundary and cleaned up
under the user's standing destructive-test authorization for seat 15.

## Remaining boundary

Physical DLNA/Chromecast rendering, cross-seat playback-owner handoff,
T480/Eagle/Surface mutating playback, and human speaker judgment remain open.
