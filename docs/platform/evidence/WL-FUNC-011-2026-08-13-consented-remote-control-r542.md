# WL-FUNC-011 — consented remote-control provider boundary (r542)

## Production result

`CallMediaProviderRegistry` now treats `StartCall(RemoteDesktop)` as a signed
invitation only. It performs no VDI provider effect. The provider receives the
exact `CallId` only after the Collaboration core has admitted an
`AnswerCall { call }`; `HangUpCall { call }` remains the exact revocation path.
An absent or substituted call has no resolved `CallKind` and therefore cannot
reach the provider.

The hostile regression registers a provider that panics on every command other
than exact answer/revocation. It proves that the invitation and a substituted
answer produce zero provider commands, then observes only the admitted call's
answer and hang-up.

## Farm gates

- BigBoy `.130`, slot 3: `cargo clippy -p mackesd --all-targets --all-features -- -D warnings` — PASS.
- BigBoy `.130`, slot 1: focused hostile regression — PASS, 1/1.
- BigBoy `.130`, slot 3: `cargo build -p mackesd --all-targets --all-features` — PASS.
- BigBoy `.130`, slot 2: exact-file `rustfmt --check` ran once and reported one
  deterministic line-wrap delta in the new regression; that exact formatter
  delta was applied without rerunning the gate.
- Scoped `git diff --check` — PASS.

Two focused-test invocations were stopped before test execution: the first had
an incomplete `--exact` filter and the second encountered a concurrent owner's
artifact lock. The successful isolated slot-1 invocation is the only execution
of the regression. No `.50` command was started.

## Residual acceptance

This slice does not claim a concrete VDI provider, live screen/control frames,
or trusted-seat UX proof. Group-media/SFU topology, screen-capture selection,
provider reconnect, and remaining legacy hard-cut work remain pre-release.
Live media and one-node acceptance remain deferred until after the first full
release under the current project directive.
