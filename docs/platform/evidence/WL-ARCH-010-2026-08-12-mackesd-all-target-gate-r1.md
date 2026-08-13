# WL-ARCH-010 mackesd all-target quality gate — 2026-08-12

## Scope

Commit `d806b8c7` clears the `mackesd` all-target strict-Clippy backlog and
repairs the deterministic failures exposed by the first integrated run. The
initial inventory contained 500 diagnostics across 97 files and 40 warning
families. The landed batch keeps the public `SessionAction::Publish(VdiSession)`
shape under one documented compatibility exception and otherwise fixes or
removes the warned code.

The review also corrected runtime and authority defects uncovered during the
cleanup: Cloud provider discovery can start from an empty registry, Android
lifecycle dispatch is serialized, VDI sources remain generation-bound, App VM
capabilities fail closed, Surface replay recovery is exact-action bound,
unsupported Surface firmware models return a typed refusal, late transfer Bus
binding retries (`7f65fc43`), and recovered Workload status cannot be paired
with another request's identity or placement contract. The SQLite-authority scanner now
excludes `cfg(test)` fixtures while its negative self-test still catches a new
production write.

## Baseline and repair

- The non-authoritative parallel harness reported 4,903 passed, 19 failed, and
  1 ignored. `mackesd` tests mutate process-global environment, so the CI
  contract requires `--test-threads=1`.
- The pre-fix canonical serial run reported 4,905 passed, 17 failed, and 1
  ignored. Focused farm lanes reproduced and repaired every failure rather than
  weakening the serial contract.
- Focused post-fix lanes passed the Display1/provisioning, Surface,
  Cloud/App-VM, chat/host/compute, transfer recovery, Nebula, node-grade, and
  Workload-ledger regressions.

## Final farm gates

- `.90`, slot `arch010-mackesd-serial-20260812`:
  `cargo clippy -p mackesd --all-targets --locked -- -D warnings` — passed.
- `.90`, same slot:
  `cargo test -p mackesd --all-targets --locked -- --test-threads=1` — core
  library 4,924 passed, 0 failed, 1 ignored in 348.62s; every binary and
  integration target passed, with the opt-in live-fleet test also ignored.
- `.196`, slot `arch010-mackesd-build-20260812`:
  `cargo build -p mackesd --locked` — passed; the recovered Workload
  request/status binding regression also passed 1/1.
- `.170`, slot `arch010-mackesd-doc-20260812`:
  `cargo test -p mackesd --doc --locked` — passed.
- `.90`/`.50`: `lint-workload-authority.sh` passed;
  `lint-mackesd-sqlite-authority.sh --self-test` passed; the final SQLite
  authority inventory passed with zero production residual sites.

## Remaining acceptance

This gate does not close WL-ARCH-010. Repository-wide strict Clippy and live
Display1/KMS/EGL presentation proof remain. No unavailable live hardware result
is represented as a pass by this record.
