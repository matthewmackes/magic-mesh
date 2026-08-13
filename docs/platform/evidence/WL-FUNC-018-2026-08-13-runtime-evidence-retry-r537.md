# WL-FUNC-018 — ordered App VM runtime-evidence retry (r537)

Date: 2026-08-13

## Gap closed

The session broker previously advanced its App VM runtime-evidence Bus cursor
before the corresponding authenticated `AppState` action was durably published.
A temporarily unavailable signer or failed Bus write therefore lost the
transition for the lifetime of the daemon, leaving Front Door readiness stale
even after authority recovered.

`session_broker` now retains one ordered Bus batch until each observation is
either durably published or definitively rejected. A deferred head blocks later
generations so `StartingApp` cannot be skipped before `Connected`. Identity
mismatches and illegal/stale lifecycle edges remain fail-closed and do not clog
the queue. Replacing the Bus clears observations from the retired store
identity rather than replaying them into a new authority boundary.

## Owned scope

- `crates/mesh/mackesd/src/workers/session_broker.rs`
- this evidence record

No Front Door shell, Android lifecycle, Collaboration, Browser VDI, generic
resource-action, worklist, or release-script path was changed.

## Farm gates

- `172.20.0.130`, slot `func018-runtime-retry-build-r1`:
  `cargo build -p mackesd --features async-services` — passed.
- `172.20.0.90`, slot `func018-runtime-retry-clippy-r1`:
  `cargo clippy -p mackesd --features async-services --all-targets -- -D warnings`
  — passed.
- `172.20.0.90`, reused slot `func018-runtime-retry-clippy-r1`:
  `cargo test -p mackesd --features async-services runtime_evidence_waits_for_signer_without_losing_lifecycle_order -- --nocapture`
  — passed 1/1 (4,974 unrelated library tests filtered out).
- `172.20.0.196`, slot `func018-runtime-retry-rustfmt-r3`: Rust 1.94
  `rustfmt --edition 2021 --check` reported only five pre-existing formatting
  regions at lines 495, 502, 2143, 2469, and 2967. It reported no diff in the
  owned implementation or regression-test hunks. The one initially reported
  owned call-site layout was corrected before this final diagnostic.
- Scoped `git diff --check` — passed.

The first `.170` test invocation compiled successfully but selected zero tests
because `--exact` was paired with an unqualified name. It is not counted as
acceptance evidence; the corrected `.90` invocation above is authoritative.

## Residual acceptance

Pre-release coding still requires an audit of remaining App VM audio,
persistence, stop/crash cleanup, and reconnect paths, plus binding the governed
App VM image/profile and approved Flatpak runtime supply into the first release.
After that release, the deferred non-blocking acceptance covers live one-node
VDI, audio, persistence, sandbox, package, crash/reconnect, cleanup, and
corrected-forward recovery proof. This slice does not claim those live results.
