# WL-FUNC-021 — Music daemon host-identity cache (2026-08-07)

## Finding

`mde-musicd::state::local_host()` spawned the `hostname` process on every
state, handoff, MPRIS, and catalog request. Those repeated process launches
were unnecessary because the daemon's local identity is immutable for its
lifetime and could amplify across seats.

## Change

The helper now resolves the hostname once through `OnceLock` and returns the
cached identity thereafter. The `localhost` fallback and returned value are
unchanged; repeated callers no longer create a subprocess.

## Verification

Farm `.50`, slot `music-state-host-cache-r1`:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-state-host-cache-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd state --locked -- --nocapture
```

Result: **20 passed, 0 failed, 165 filtered out**. The new regression confirms
the identity remains stable across repeated calls; all state, handoff, and
bus responder tests in the filtered target also passed.

Live multi-seat CPU proof remains open until Dell routes return.
