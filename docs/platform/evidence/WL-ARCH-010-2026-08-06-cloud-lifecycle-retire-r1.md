# WL-ARCH-010 — cloud lifecycle retirement (2026-08-06)

## Outcome

The cloud worker no longer classifies or dispatches VM `instance-*` or rootless
container `container-*` lifecycle verbs. The stale `container_lifecycle.rs`
systemd/journalctl adapter was removed. Unsupported legacy topics fail with an
unknown-verb response before authorization, replay consumption, or backend
execution. The isolated Cuttlefish provider lifecycle seam remains separate and
is not a cloud action publisher.

The Front Door and Explorer instance lifecycle paths now publish only the typed
`action/workload/operation` contract with capability-bound Workload requests.
Runtime source search found no remaining `action/cloud/instance-*` or
`action/cloud/container-*` publisher/reader outside refusal and historical
contract tests.

## Verification

- BigBoy `.130`, slot `arch010-cloud-lifecycle-retire-20260806-r2`:
  `cargo test --locked -p mackesd workers::cloud::verbs::tests:: -- --nocapture` — 6/6.
- BigBoy `.130`, slot `arch010-cloud-lifecycle-retire-20260806-r3`:
  `cargo test --locked -p mackesd legacy_lifecycle_topics -- --nocapture` — 2/2.
- `.90` slot `arch010-shell-lifecycle-20260806-r2` reached ENOSPC during
  compilation; it produced no test failure and was removed. The shell lane was
  rerouted to `.50`.
- `.50`, slot `arch010-shell-lifecycle-20260806-r3`:
  `cargo test --locked -p mde-shell-egui lifecycle -- --nocapture` — 27/27.
- `./install-helpers/lint-workload-authority.sh` — clean.
- `git diff --check` — clean.

## Source hashes

```text
crates/mesh/mackesd/src/workers/cloud/verbs.rs
57678b659adb42eb05632284a85960db80d729f5df2f4cac20c8c0ee862343cf
crates/desktop/mde-shell-egui/src/front_door.rs
f24d63a70e296400c1f15fe684cd580eb6b815128cc305fd281f8ac38f32e03b
crates/desktop/mde-shell-egui/src/explorer/mod.rs
602bc7fe1907fed537c6ae5abbf5bb06a3ccd3126fb680386d03db2d86200101
crates/desktop/mde-shell-egui/src/main.rs
0adb5f9085169d7f08466be7d50b5a3b8c6d49c481f8ea3fe1fe6065e2d761be
```

Dell runtime was not changed; this is source/evidence review material only.
