# WL-ARCH-010 cloud and certificate authority hard cut — 2026-08-09

## Outcome

The cloud lane no longer owns VM creation or lifecycle. `CloudRunner` retains
read-only inventory, dry-run planning, and Ansible configuration, but its
OpenTofu apply and direct `virsh` lifecycle methods are deleted. The retained
`action/cloud/provision` verb returns an explicit no-effect refusal before
authorization consumption, mutable rendering, or backend contact. Android
outer-VM lifecycle likewise fails closed until it can delegate to the typed
Workload operation lane; Cuttlefish inventory, guest launch, readiness, and VDI
source contracts remain without a direct outer-VM actuator.

The shell no longer publishes cloud provision or offers Provision Apply. It can
save desired state and run a dry plan, then directs the operator to the typed
Workload row operation. Ansible Configure remains armed and live. The authority
lint now rejects restoration of the deleted runner methods, cloud provision
publisher/GUI symbols, Android runner call, Cuttlefish lifecycle method, and
retired responder registrations.

The producerless `cert_authority` responder, topic drain, registry entry, and 22
contract-only tests were deleted. Enrollment continues to use the canonical
sealed-CA path; no publisher or API depended on the responder. Tests that only
simulated effects through the deleted cloud/Cuttlefish lifecycle seams were
removed with those seams; retained authorization, parsing, observation,
readiness, desired-state, and explicit-refusal behavior remains covered.

## Farm verification

- BigBoy (`172.20.0.130`), slot `arch010-cloud-cut-daemon-r1`:
  `cargo test -p mackesd --lib --features async-services workers::cloud --locked -- --nocapture`
  passed 201/201.
- Machine 9 (`172.20.0.50`), slot `arch010-cloud-cut-shell-r1`:
  `cargo test -p mde-shell-egui iac::tests --locked -- --nocapture` passed
  57/57.
- Machine 193 (`172.20.0.90`), slot `arch010-cloud-cut-registry-r1`: both exact
  worker-census and growing-rank-superset tests passed 1/1.
- Machine 194 (`172.20.0.170`), slot `arch010-cloud-cut-check-r1`:
  `cargo check -p mackesd --lib --features async-services --locked` passed.
- Machine 196 (`172.20.0.196`), slot `arch010-cloud-cut-lints-r1`: scoped
  Rust formatting, authority self-test/live lint, worklist lint, and document
  supersession lint passed. The active worklist remains honest at 18 Remaining.

## Source hashes

```text
819a839e40e504406b36a24485cbf390519bf1062c67e1bdd723cc6037155a49  crates/desktop/mde-shell-egui/src/iac/mod.rs
7985592a9de47639f4811d2004de4d10bb0c068db7a7ab207dc6e6ee4f6c5720  crates/desktop/mde-shell-egui/src/iac/provision_form.rs
ba5ec92b7166f531cc05fe9cbf1f914627487e18deeff120aeb3c8816a699e79  crates/mesh/mackesd/src/workers/cloud/mod.rs
d840340bf2a43ee4c271f32e7764830cf8be5ee7cc2c4dee08454025add641dd  crates/mesh/mackesd/src/workers/cloud/runner.rs
707d6c07be7a65e1b006c6da1144d4e051df17458e5a14d792c628d40127a236  crates/mesh/mackesd/src/workers/cloud/verbs.rs
bcabc327926decee8013ebd4c6883db79772722f2ac64c146d81bfb92f747af4  crates/mesh/mackesd/src/workers/cloud/verbs/android_lifecycle.rs
6e5f3d0f7214a50974b8fa70058faf3d8c50583b19976fa175ee94d55c142951  crates/mesh/mackesd/src/workers/cloud/verbs/cuttlefish.rs
eabf53aca6ff72d08b4c6d7352d773e7e4c5070b70113e6a1378cb001b9e9b2e  install-helpers/lint-workload-authority.sh
```

## Remaining boundary

This removes another competing-authority cluster but does not close
WL-ARCH-010. Typed lifecycle integration, native attachment, package/live-seat
proof, and the remaining authority audit still keep the epic `Remaining`.
