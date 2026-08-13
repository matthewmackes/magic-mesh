# WL-FUNC-018 App VM Workload readiness boundary — r499

Date: 2026-08-13

## Result

The universal resource adapter no longer treats signed Flatpak catalog validity
as runtime availability. App cards now join against the existing typed
`WorkloadStateSnapshot` authority and require one fresh VM row that binds the
serving node, App ID, catalog revision, and admitted guest-profile image.

Missing Workload state, stale state, a substituted App identity, a substituted
guest profile, or ambiguous matching rows keep the App inspectable but
unavailable and non-launchable. Only an exact `Ready` phase, `Running` power,
and `Ready` guest row publishes `Available` with a generation-bound
`launch-gN` action. No parallel readiness state or provider probe was added.

## Farm gates

- `.90`, slot `func018-app-readiness-test-r499f`: exact hostile focused
  regression passed 1/1 (`4,949` filtered). An earlier unqualified `--exact`
  invocation on BigBoy selected zero tests and was rejected as evidence; its
  superseded rerun was stopped when the gate was explicitly rerouted to `.90`.
- `.196`, slot `func018-resource-adapter-module-r499`: the complete relevant
  adapter module passed 18/18 (`4,932` filtered).
- `.90`, slot `func018-app-readiness-clippy-r499`: strict
  `cargo clippy -p mackesd --lib --features async-services -- -D warnings`
  passed.
- `.196`, slot `func018-app-readiness-fmt-r499e`: exact-file
  `rustfmt --edition 2021 --check` reported only four pre-existing regions
  outside this slice; every r499 implementation and test hunk is rustfmt-clean.
  The broader package-format attempt was not claimed because it also exposed
  unrelated repository drift.

## Remaining acceptance

The first-release build still needs the current App VM image/profile artifacts
and package gates. Installed App VM boot/readiness, VDI attachment, reconnect,
cleanup, sandbox, persistence, and upgrade proof remains deferred and
non-blocking until after that release under the active worklist policy.
