# WL-FUNC-019 — generation-bound Workload actions in Remote Sessions (r530)

Remote Sessions now routes the existing VM/container `Start` and `Resume`
cards through the typed resource-action ingress and Workloads authority. The
shell derives node, workload ID, operation, and generation only from one
admitted full card, then reopens the authoritative Workload projection at the
click boundary. A missing identity, changed generation, changed VM/container
backend class, ambiguous action, stale lease, or unavailable Bus fails closed.
No raw command, path, topic, endpoint, or caller-selected authority is emitted.

The accepted router reply remains bound to the exact catalog digest, resource,
action, generation, cancellation identity, and fixed
`action/workload/operation` route. The existing correlated cancellation ledger
therefore also applies to these newly reachable Workload actions.

## Farm evidence

- BigBoy `.130`, slot `func019-workload-route-test-r530`:
  `cargo test --locked -p mde-shell-egui
  vm_resume_routes_only_after_exact_workload_generation_revalidation --
  --nocapture` passed 1/1. The fixture proves exact VM Resume routing and rejects
  a replacement generation and duplicate action identity.
- `.170`, slot `func019-workload-route-bin-clippy-r530`:
  `cargo clippy --locked -p mde-shell-egui --bin mde-shell-egui -- -D warnings`
  passed.
- `.50`, slot `func019-workload-route-fmt-r530`:
  `cargo fmt -p mde-shell-egui -- --check` passed.
- The stronger `.170` all-target Clippy reached unrelated concurrent warnings
  in `car_keymap.rs`, `status_bar.rs`, and `system/mesh.rs`; those files are
  outside this slice and were preserved. Production binary Clippy is green.
- Local `git diff --check` passed.

## Remaining FUNC-019 acceptance

Route/capture coverage for the other universal resource kinds and deferred
post-release one-node loss/rejoin/action recovery proof remain. This slice does
not claim live proof or epic closure.
