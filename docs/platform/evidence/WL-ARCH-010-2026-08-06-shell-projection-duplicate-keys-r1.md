# WL-ARCH-010 shell Workload projection duplicate-key guard — 2026-08-06

`mde-shell-egui` now rejects duplicate JSON object keys before decoding the
node-local `state/workloads/<node>` projection. The shell continues to publish
only typed, capability-bound operations and cannot treat a hostile duplicate
`node` field as authoritative UI state.

Verification:

- Hostile duplicate top-level `node` coverage was added; `git diff --check`
  passed.
- Farm `.50`, slot `arch010-shell-authority-duplicate-20260806-r1`, attempted
  `cargo test -p mde-shell-egui workload_api::tests:: -- --nocapture` but the
  host was full (`ENOSPC`) during compilation. No passing result is claimed.
- Source SHA-256:
  `dcaf1486ed8449f0e76ecad17f703d478d0298060dead9ba6eeec388d4112456`.

This is a projection-boundary proof only. Live seat rendering, mutation
acceptance, restart/recovery, and Dell acceptance remain open. Dell runtime was
not modified.
