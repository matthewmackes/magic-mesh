# WL-UX-011 module-reload provider coverage — r530

Date: 2026-08-13

## Result

The node device-control worker no longer treats a module-wide
`rmmod`/`modprobe` bounce as though it affected only the selected device. For an
authorized `ReloadModule` request, the worker reads the exact inventory
generation selected by the operator and admits the action only when that
generation contains exactly one device bound to the requested module.

An absent module or a second provider-owned consumer produces a bounded,
truthful refusal before authorization can reach the fixed command executor.
The same provider-scope admission runs again after exact-body capability
verification, immediately before execution, so an inventory-generation change
revokes the staged action. Every refusal continues through the existing
hash-chained action audit and failure notification path. No shell/UI or direct
hardware mutation seam was added.

## Owned files

- `crates/mesh/mackesd/src/workers/device_control.rs`
- `docs/platform/evidence/WL-UX-011-2026-08-13-module-reload-provider-coverage-r530.md`

## Farm gates

- `.170`, slot `ux011-module-scope-test-r530b`:
  `cargo test -p mackesd shared_module_reload_is_refused_by_exact_provider_generation_and_audited -- --nocapture`
  passed 1/1. The regression exercises a signed, generation-bound request,
  shared-module refusal, and durable audit evidence without invoking hardware.
- `.170`, slot `ux011-module-scope-fmt-r530`:
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/device_control.rs`
  passed.
- `.130`, slot `ux011-module-scope-clippy-r530`:
  `cargo clippy -p mackesd --all-targets -- -D warnings` reached and accepted
  the changed module after its slice-local dead-code finding was corrected. The
  crate gate remains red solely in concurrently owned excluded file
  `crates/mesh/mackesd/src/workers/collab_media.rs:674` for
  `clippy::enum_variant_names`; no suppression or cross-scope edit was made.
- Local `git diff --check` passed.

The initial focused-test process on `.50` was stopped after an ownership check
showed that two unrelated jobs already occupied that host. The gate was moved
to `.170`; no result from the interrupted lane is claimed.

## Acceptance boundary

This proves the software refusal and audit boundary only. It does not claim a
live module reload or fleet hardware proof. Remaining UX-011 provider coverage,
other safe-action adapters, and deferred post-release one-node acceptance stay
open in the canonical worklist.
