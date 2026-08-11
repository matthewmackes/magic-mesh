# Mirror restart readiness evidence — 2026-08-11

- Scope: mirror-sync startup retracts retained DNF repo advertisements before
  they can imply that replicated bytes are current.
- Readiness: a leader republishes after a successful authoritative sync; a
  follower republishes only after observing a strictly forward replicated
  generation. Failed sync or retraction remains unavailable and retries.
- Hostile regression: retained readiness plus an injected upstream failure
  remains retracted through restart rather than serving stale mirror state.
- Farm gate: BigBoy `.130`, slot 2: **1 passed, 0 failed, 4,824 filtered**.
- Rustfmt and scoped `git diff --check`: passed.
