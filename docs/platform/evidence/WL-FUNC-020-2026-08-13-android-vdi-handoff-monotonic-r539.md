# WL-FUNC-020 Android VDI handoff monotonicity — 2026-08-13

## Result

The Android-specific Workloads-to-Remote-Sessions handoff now preserves the
newest exact readiness authority while the shell is between surfaces. A delayed
older Cuttlefish generation for the same placement and workload can no longer
replace the generation/session the operator selected. A same-generation source
whose session, mesh host, provenance, digest, or lifetime is substituted is
also refused. Selecting a different workload remains an intentional replacement
because its generation counter belongs to a separate authority stream.

Changed production and regression scope:

- `crates/desktop/mde-shell-egui/src/iac/mod.rs`
- `crates/desktop/mde-shell-egui/src/iac/tests.rs`

The hostile regression queues generation 7, then attempts both a delayed
generation 6 source and a same-generation session/host substitution. The one
consumed handoff remains byte-for-byte equal to the original generation 7
source, and the slot remains one-shot.

## Farm gates

- BigBoy `.130`, slot 2: `cargo test -p mde-shell-egui android_vdi_handoff_rejects_delayed_or_substituted_source_before_attachment -- --nocapture` — passed 1/1 (1,606 filtered out).
- `.90`, slot 2: `cargo build -p mde-shell-egui --all-targets` — passed.
- `.90`, slot 2: `cargo fmt -p mde-shell-egui -- --check` — passed after applying the one owned rustfmt change.
- BigBoy `.130`, slot 2: `cargo clippy -p mde-shell-egui --all-targets --all-features -- -D warnings` — the Android handoff code reached package checking without a diagnostic, but the package gate is pre-existing red at `crates/desktop/mde-shell-egui/src/communications/mod.rs:608` (`clippy::while_let_loop`). That concurrently owned file is outside this slice and was not changed. The same exact single failure reproduced once after the concurrent branch advanced; no further loop was run.
- Scoped `git diff --check` — passed.

## Remaining acceptance

FUNC-020 still requires the first release to consume the signed Cuttlefish image
and deterministic guest packages, plus the remaining concrete seat-side decoder
and guest-input integration. After that release, the deferred non-blocking
one-node nested-KVM, lifecycle, VDI input/audio/reconnect, isolation, upgrade,
and live UX acceptance must run. This slice claims no live transport or release
proof.
